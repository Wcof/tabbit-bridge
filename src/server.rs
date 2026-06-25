//! HTTP 服务：tiny_http 同步栈，路由 + 鉴权 + Host 校验 + CORS/PNA。
//! 处理顺序：Host 校验 → OPTIONS 预检短路 → Bearer 校验 → action 路由。
//! 全程不把 token 或命令输出写入日志。

use std::sync::Arc;
use std::time::Duration;

use subtle::ConstantTimeEq;
use tiny_http::{Header, Method, Response, Server, StatusCode};

use crate::config::Config;
use crate::exec::{self, ExecOutcome};
use crate::registry;

const VERSION: &str = env!("CARGO_PKG_VERSION");

pub struct AppState {
    pub port: u16,
    pub token: Arc<String>,
    pub timeout: Duration,
    pub max_output: usize,
}

/// 启动 HTTP 服务（阻塞当前线程）。
pub fn serve(cfg: &Config) -> std::io::Result<()> {
    let bind = format!("{}:{}", cfg.server.bind, cfg.server.port);
    let server = Server::http(&bind).map_err(|e| {
        std::io::Error::new(std::io::ErrorKind::AddrInUse, format!("bind {bind} 失败: {e}"))
    })?;
    eprintln!("[tabbit-bridge] 监听 {}", bind); // 不含 token

    let state = Arc::new(AppState {
        port: cfg.server.port,
        token: Arc::new(cfg.server.token.clone()),
        timeout: Duration::from_millis(cfg.limits.timeout_ms),
        max_output: cfg.limits.max_output_bytes,
    });

    for request in server.incoming_requests() {
        // 第一层：Host 头校验（防 DNS 重绑定）
        if !host_is_valid(&request, state.port) {
            respond_text(request, StatusCode::from(403), "invalid host");
            continue;
        }

        // OPTIONS 预检短路 + CORS/PNA
        if request.method() == &Method::Options {
            handle_preflight(request);
            continue;
        }

        let path = request.url().to_string();

        // 路由
        match path.as_str() {
            "/healthz" => handle_health(request),
            "/v1/exec" => {
                // 第二层：Bearer 鉴权（常量时间比较）
                if !bearer_ok(&request, &state.token) {
                    respond_text(request, StatusCode::from(401), "unauthorized");
                    continue;
                }
                handle_exec(request, &state);
            }
            _ => respond_text(request, StatusCode::from(404), "not found"),
        }
    }
    Ok(())
}

fn host_is_valid(req: &tiny_http::Request, port: u16) -> bool {
    let h = req
        .headers()
        .iter()
        .find(|x| x.field.equiv("Host"))
        .and_then(|x| x.value.as_str().parse::<String>().ok());
    let Some(h) = h else {
        return false;
    };
    // 接受 127.0.0.1:port / localhost:port / [::1]:port
    let valid = [
        format!("127.0.0.1:{port}"),
        format!("localhost:{port}"),
        format!("[::1]:{port}"),
    ];
    let h_trim = h.trim();
    // 也不带端口的 localhost/127.0.0.1（部分浏览器对非标准端口可能省略）
    let bare = ["127.0.0.1", "localhost", "[::1]"];
    valid.iter().any(|v| *v == h_trim) || bare.iter().any(|b| *b == h_trim)
}

fn bearer_ok(req: &tiny_http::Request, expected: &str) -> bool {
    let Some(h) = req
        .headers()
        .iter()
        .find(|x| x.field.equiv("Authorization"))
    else {
        return false;
    };
    let v = h.value.as_str();
    let v = v.trim();
    let Some(rest) = v.strip_prefix("Bearer ") else {
        return false;
    };
    let got = rest.trim().as_bytes();
    let want = expected.as_bytes();
    if got.len() != want.len() {
        // 长度不同也要做一次比较以维持常量时间语义
        let _ = got.ct_eq(want);
        return false;
    }
    got.ct_eq(want).into()
}

fn handle_preflight(req: tiny_http::Request) {
    let mut resp = Response::empty(204);
    add_cors_headers(&mut resp, &req);
    let _ = req.respond(resp);
}

fn add_cors_headers<R: std::io::Read>(resp: &mut Response<R>, _req: &tiny_http::Request) {
    resp.add_header(
        Header::from_bytes(&b"Access-Control-Allow-Origin"[..], &b"*"[..]).unwrap(),
    );
    resp.add_header(
        Header::from_bytes(
            &b"Access-Control-Allow-Methods"[..],
            &b"GET, POST, OPTIONS"[..],
        )
        .unwrap(),
    );
    resp.add_header(
        Header::from_bytes(
            &b"Access-Control-Allow-Headers"[..],
            &b"Authorization, Content-Type"[..],
        )
        .unwrap(),
    );
    resp.add_header(
        Header::from_bytes(
            &b"Access-Control-Allow-Private-Network"[..],
            &b"true"[..],
        )
        .unwrap(),
    );
}

fn handle_health(req: tiny_http::Request) {
    let body = serde_json::json!({ "status": "ok", "version": VERSION });
    let mut resp = Response::from_string(body.to_string());
    resp.add_header(
        Header::from_bytes(&b"Content-Type"[..], &b"application/json"[..]).unwrap(),
    );
    let _ = req.respond(resp);
}

fn handle_exec(mut req: tiny_http::Request, state: &AppState) {
    if req.method() != &Method::Post {
        respond_text(req, StatusCode::from(405), "method not allowed");
        return;
    }
    let mut body = Vec::with_capacity(4096);
    if let Err(_) = std::io::Read::read_to_end(req.as_reader(), &mut body) {
        respond_text(req, StatusCode::from(400), "bad request");
        return;
    }    let parsed: serde_json::Value = match serde_json::from_slice(&body) {
        Ok(v) => v,
        Err(_) => {
            respond_text(req, StatusCode::from(400), "invalid json");
            return;
        }
    };
    let action = parsed
        .get("action")
        .and_then(|v| v.as_str())
        .unwrap_or("");

    // 第三层：白名单
    let Some(entry) = registry::resolve(action) else {
        respond_text(req, StatusCode::from(404), "action not found");
        return;
    };

    // 受控参数（可选）
    let controlled_value = if let Some((field, _regex, _flag)) = entry.controlled {
        parsed.get(field).and_then(|v| v.as_str())
    } else {
        None
    };

    let Some(cmd) = registry::build_command(&entry, controlled_value) else {
        respond_text(req, StatusCode::from(400), "controlled parameter invalid");
        return;
    };

    // 第四层：安全执行
    let outcome: ExecOutcome = exec::run(cmd, state.timeout, state.max_output);
    let mut stdout = outcome.stdout;
    let mut stderr = outcome.stderr;
    if outcome.truncated {
        exec::append_truncation_marker(&mut stdout);
        exec::append_truncation_marker(&mut stderr);
    }

    // 组装统一响应
    let raw_str = String::from_utf8_lossy(&stdout).to_string();
    let stderr_str = String::from_utf8_lossy(&stderr).to_string();

    let data: serde_json::Value = if outcome.exit_code == Some(0) && entry.parses_json {
        serde_json::from_slice(&stdout)
            .map_err(|_| serde_json::Value::Null)
            .unwrap_or(serde_json::Value::Null)
    } else {
        serde_json::Value::Null
    };

    let raw_field = if entry.parses_json && data.is_array() || data.is_object() {
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
        StatusCode::from(408)
    } else if outcome.exit_code != Some(0) {
        StatusCode::from(500)
    } else {
        StatusCode::from(200)
    };

    let mut resp = Response::from_string(resp_body.to_string())
        .with_status_code(status.0);
    resp.add_header(
        Header::from_bytes(&b"Content-Type"[..], &b"application/json"[..]).unwrap(),
    );
    let _ = req.respond(resp);
}

fn respond_text(req: tiny_http::Request, code: StatusCode, msg: &str) {
    let body = serde_json::json!({ "error": msg, "code": code.0 });
    let mut resp = Response::from_string(body.to_string())
        .with_status_code(code.0);
    resp.add_header(
        Header::from_bytes(&b"Content-Type"[..], &b"application/json"[..]).unwrap(),
    );
    let _ = req.respond(resp);
}
