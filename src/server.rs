//! HTTP 服务：tiny_http 同步栈，路由 + 鉴权 + Host 校验 + CORS/PNA。
//! 处理顺序：Host 校验 → OPTIONS 预检短路 → Bearer 校验 → action 路由。
//! 全程不把 token 或命令输出写入日志。

use std::sync::mpsc;
use std::sync::{Arc, Mutex};
use std::thread;
use std::time::{Duration, Instant};
use std::io::Read;

use subtle::ConstantTimeEq;
use tiny_http::{Header, Method, Response, Server};

use crate::config;
use crate::config::Config;
use crate::exec::{self, ExecOutcome};
use crate::registry;
use crate::updater;

const VERSION: &str = env!("CARGO_PKG_VERSION");
const MAX_BODY: u64 = 64 * 1024;

pub struct AppState {
    pub port: u16,
    pub token: Arc<String>,
    pub timeout: Duration,
    pub max_output: usize,
    pub rate_per_min: u32,
    pub bucket: Mutex<(Instant, u32)>,
    pub update_available: Mutex<Option<String>>,
    pub tools: config::ToolsCfg,
}

/// 简易线程池：N 个 worker 从 channel 取任务执行，慢命令不再阻塞 healthz。
fn spawn_worker_pool(size: usize) -> mpsc::Sender<Box<dyn FnOnce() + Send + 'static>> {
    let (tx, rx) = mpsc::channel::<Box<dyn FnOnce() + Send + 'static>>();
    let rx = Arc::new(Mutex::new(rx));
    for _ in 0..size {
        let rx = rx.clone();
        thread::spawn(move || loop {
            let job = { rx.lock().unwrap().recv() };
            match job {
                Ok(j) => j(),
                Err(_) => break,
            }
        });
    }
    tx
}

pub fn serve(cfg: &Config) -> std::io::Result<()> {
    let bind = format!("{}:{}", cfg.server.bind, cfg.server.port);
    let server = Server::http(&bind).map_err(|e| {
        std::io::Error::new(std::io::ErrorKind::AddrInUse, format!("bind {bind} 失败: {e}"))
    })?;
    eprintln!("[tabbit-bridge] 监听 {}", bind);

    let state = Arc::new(AppState {
        port: cfg.server.port,
        token: Arc::new(cfg.server.token.clone()),
        timeout: Duration::from_millis(cfg.limits.timeout_ms),
        max_output: cfg.limits.max_output_bytes,
        rate_per_min: cfg.limits.rate_per_min,
        bucket: Mutex::new((Instant::now(), 0)),
        update_available: Mutex::new(None),
        tools: cfg.tools.clone(),
    });

    // 启动后台检查：仅查询并写入 config.toml + AppState，不自动安装
    if cfg.update.check_on_start {
        let state2 = state.clone();
        thread::spawn(move || {
            if let Ok(r) = updater::check_latest() {
                if updater::is_newer(&r.version, updater::current_version()) {
                    let _ = config::record_latest(&r.version);
                    if let Ok(mut g) = state2.update_available.lock() {
                        *g = Some(r.version);
                    }
                }
            }
        });
    }

    let pool = spawn_worker_pool(4);
    for request in server.incoming_requests() {
        let state = state.clone();
        let _ = pool.send(Box::new(move || handle_request(request, &state)));
    }
    Ok(())
}

fn handle_request(req: tiny_http::Request, state: &AppState) {
    if !host_is_valid(&req, state.port) {
        send_json(req, 403, &serde_json::json!({"error":"invalid host","code":403}));
        return;
    }
    if req.method() == &Method::Options {
        handle_preflight(req);
        return;
    }
    let path = req.url().to_string();
    match path.as_str() {
        "/healthz" => handle_health(req, &state),
        "/v1/exec" => {
            if !bearer_ok(&req, &state.token) {
                send_json(req, 401, &serde_json::json!({"error":"unauthorized","code":401}));
                return;
            }
            handle_exec(req, &state);
        }
        _ => send_json(req, 404, &serde_json::json!({"error":"not found","code":404})),
    }
}

fn host_is_valid(req: &tiny_http::Request, port: u16) -> bool {
    let Some(h) = req.headers().iter().find(|x| x.field.equiv("Host")) else {
        return false;
    };
    let h = h.value.as_str().trim().to_string();
    let valid = [
        format!("127.0.0.1:{port}"),
        format!("localhost:{port}"),
        format!("[::1]:{port}"),
    ];
    valid.iter().any(|v| *v == h)
}

fn bearer_ok(req: &tiny_http::Request, expected: &str) -> bool {
    let Some(h) = req.headers().iter().find(|x| x.field.equiv("Authorization")) else {
        return false;
    };
    let v = h.value.as_str().trim();
    let Some(rest) = v.strip_prefix("Bearer ") else {
        return false;
    };
    let got = rest.trim().as_bytes();
    let want = expected.as_bytes();
    got.len() == want.len() && bool::from(got.ct_eq(want))
}

/// 统一 CORS 头注入：所有响应都必须经过它，否则浏览器会拦截。
fn add_cors<R: std::io::Read>(resp: &mut Response<R>) {
    resp.add_header(Header::from_bytes(&b"Access-Control-Allow-Origin"[..], &b"*"[..]).unwrap());
    resp.add_header(Header::from_bytes(&b"Access-Control-Allow-Methods"[..], &b"GET, POST, OPTIONS"[..]).unwrap());
    resp.add_header(Header::from_bytes(&b"Access-Control-Allow-Headers"[..], &b"Authorization, Content-Type"[..]).unwrap());
    resp.add_header(Header::from_bytes(&b"Access-Control-Allow-Private-Network"[..], &b"true"[..]).unwrap());
    resp.add_header(Header::from_bytes(&b"Vary"[..], &b"Origin, Access-Control-Request-Headers"[..]).unwrap());
}

fn send_json(req: tiny_http::Request, status: u16, body: &serde_json::Value) {
    let mut resp = Response::from_string(body.to_string()).with_status_code(status);
    resp.add_header(Header::from_bytes(&b"Content-Type"[..], &b"application/json"[..]).unwrap());
    add_cors(&mut resp);
    let _ = req.respond(resp);
}

fn handle_preflight(req: tiny_http::Request) {
    let mut resp = Response::empty(204);
    add_cors(&mut resp);
    let _ = req.respond(resp);
}

/// 剥离 ANSI 颜色/控制序列（CSI ... m 与 OSC ... BEL/ST）。
/// 仅用于 parses_json=false 的命令 stdout（如 rtk --version），让前端拿到干净文本。
/// 实现走简易状态机，不引入 regex/strip-ansi-escapes crate。
fn strip_ansi(s: &str) -> String {
    let bytes = s.as_bytes();
    let mut out = Vec::with_capacity(bytes.len());
    let mut i = 0;
    while i < bytes.len() {
        let b = bytes[i];
        // ESC = 0x1B
        if b == 0x1B {
            // CSI: ESC [ ... 终止于 0x40..0x7E
            if i + 1 < bytes.len() && bytes[i + 1] == b'[' {
                i += 2;
                while i < bytes.len() && !(0x40..0x7E).contains(&bytes[i]) {
                    i += 1;
                }
                if i < bytes.len() {
                    i += 1; // 跳过终止字节
                }
                continue;
            }
            // OSC: ESC ] ... 终止于 BEL(0x07) 或 ST(ESC \)
            if i + 1 < bytes.len() && bytes[i + 1] == b']' {
                i += 2;
                while i < bytes.len() {
                    if bytes[i] == 0x07 {
                        i += 1;
                        break;
                    }
                    if bytes[i] == 0x1B && i + 1 < bytes.len() && bytes[i + 1] == b'\\' {
                        i += 2;
                        break;
                    }
                    i += 1;
                }
                continue;
            }
            // 其他 ESC 序列：跳过 ESC 与下一字节
            i += 2;
            continue;
        }
        out.push(b);
        i += 1;
    }
    String::from_utf8_lossy(&out).into_owned()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn strip_ansi_removes_csi_and_osc() {
        // CSI 颜色码
        let s = "\x1b[32mrtk 1.2.3\x1b[0m";
        assert_eq!(strip_ansi(s), "rtk 1.2.3");
        // OSC 标题（BEL 结束）
        let s = "\x1b]0;title\x07rtk";
        assert_eq!(strip_ansi(s), "rtk");
        // OSC 标题（ST 结束）
        let s = "\x1b]2;win\x1b\\rtk";
        assert_eq!(strip_ansi(s), "rtk");
        // 无 ANSI 的纯文本不变
        assert_eq!(strip_ansi("plain text"), "plain text");
        // 多段混合
        let s = "\x1b[1;33mrtk\x1b[0m \x1b]0;t\x07v1\x1b[0m";
        assert_eq!(strip_ansi(s), "rtk v1");
    }
}

fn handle_health(req: tiny_http::Request, state: &AppState) {
    let latest = state.update_available.lock().map(|g| g.clone()).unwrap_or(None);
    let mut body = serde_json::json!({ "status": "ok", "version": VERSION });
    if let Some(v) = latest {
        body["update_available"] = serde_json::Value::Bool(true);
        body["latest_known"] = serde_json::Value::String(v);
    } else {
        body["update_available"] = serde_json::Value::Bool(false);
    }
    send_json(req, 200, &body);
}

fn handle_exec(mut req: tiny_http::Request, state: &AppState) {
    if req.method() != &Method::Post {
        send_json(req, 405, &serde_json::json!({"error":"method not allowed","code":405}));
        return;
    }
    // 限流：60 秒窗口内最多 rate_per_min 次
    {
        let mut b = state.bucket.lock().unwrap();
        let now = Instant::now();
        if now.duration_since(b.0).as_secs() >= 60 {
            *b = (now, 0);
        }
        if b.1 >= state.rate_per_min {
            drop(b);
            send_json(req, 429, &serde_json::json!({"error":"rate limited","code":429}));
            return;
        }
        b.1 += 1;
    }
    // 请求体上限 64KB，防止恶意超大 body 触发 OOM
    let mut limited = req.as_reader().take(MAX_BODY);
    let mut body = Vec::with_capacity(1024);
    if std::io::Read::read_to_end(&mut limited, &mut body).is_err() {
        send_json(req, 400, &serde_json::json!({"error":"bad request","code":400}));
        return;
    }
    let parsed: serde_json::Value = match serde_json::from_slice(&body) {
        Ok(v) => v,
        Err(_) => {
            send_json(req, 400, &serde_json::json!({"error":"invalid json","code":400}));
            return;
        }
    };
    let action = parsed.get("action").and_then(|v| v.as_str()).unwrap_or("");

    let Some(entry) = registry::resolve(action) else {
        send_json(req, 404, &serde_json::json!({"error":"action not found","code":404}));
        return;
    };

    let Some(cmd) = registry::build_command(&entry, &parsed, &state.tools) else {
        send_json(req, 400, &serde_json::json!({"error":"controlled parameter invalid","code":400}));
        return;
    };

    // ccusage 冷启动 + 多 source 扫描较慢，放宽超时到 4 倍（默认 5s → 20s）。
    // 仅对 program == "ccusage" 生效，其他命令保持原超时。
    let timeout = if entry.program == "ccusage" {
        Duration::from_millis(state.timeout.as_millis() as u64 * 4)
    } else {
        state.timeout
    };
    let outcome: ExecOutcome = exec::run(cmd, timeout, state.max_output);
    let mut stdout = outcome.stdout;
    let mut stderr = outcome.stderr;
    if outcome.truncated {
        exec::append_truncation_marker(&mut stdout);
        exec::append_truncation_marker(&mut stderr);
    }

    let raw_str = String::from_utf8_lossy(&stdout).to_string();
    let stderr_str = String::from_utf8_lossy(&stderr).to_string();

    // 对非 JSON 输出（如 rtk --version）剥离 ANSI 颜色/控制序列，避免前端 innerHTML 渲染乱码。
    let raw_str = if entry.parses_json {
        raw_str
    } else {
        strip_ansi(&raw_str)
    };

    let data: serde_json::Value = if outcome.exit_code == Some(0) && entry.parses_json {
        serde_json::from_slice(&stdout).unwrap_or(serde_json::Value::Null)
    } else {
        serde_json::Value::Null
    };

    // BUG-4 修复：括号正确包裹整个布尔表达式
    let raw_field = if entry.parses_json && (data.is_array() || data.is_object()) {
        serde_json::Value::Null
    } else {
        serde_json::Value::String(raw_str.clone())
    };

    let resp_body = serde_json::json!({
        "ok": outcome.exit_code == Some(0) && !outcome.timed_out,
        "action": action,
        "exit_code": outcome.exit_code,
        "data": data,
        "raw": raw_field,
        "stderr": stderr_str,
        "duration_ms": outcome.duration_ms,
        "truncated": outcome.truncated,
    });

    let status = if outcome.timed_out {
        408
    } else if outcome.exit_code != Some(0) {
        500
    } else {
        200
    };
    send_json(req, status, &resp_body);
}
