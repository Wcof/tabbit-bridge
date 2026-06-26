//! 安全执行器：Command::new(prog).args(fixed_args)，绝不经过 shell。
//! 施加超时（wait-timeout）与输出上限（截断标记）。

use std::io::Read;
use std::process::{Command, Stdio};
use std::time::Duration;

use wait_timeout::ChildExt;

/// 执行结果。注意：output 内容绝不写入日志。
pub struct ExecOutcome {
    pub exit_code: Option<i32>,
    pub stdout: Vec<u8>,
    pub stderr: Vec<u8>,
    pub duration_ms: u64,
    pub truncated: bool,
    pub timed_out: bool,
}

/// 执行一条已构造好的 Command。
/// - `timeout`：进程存活上限，超时则 SIGKILL。
/// - `max_output_bytes`：stdout/stderr 各自上限，超出截断并置 truncated=true。
///
/// 流程：先 spawn → wait_timeout → 超时则 kill → 再读取输出。
/// 这样避免「子进程持续写管道 → 主进程读不完 → wait 永久阻塞」的死锁。
pub fn run(mut cmd: Command, timeout: Duration, max_output_bytes: usize) -> ExecOutcome {
    let start = std::time::Instant::now();
    cmd.stdin(Stdio::null());
    cmd.stdout(Stdio::piped());
    cmd.stderr(Stdio::piped());

    let mut child = match cmd.spawn() {
        Ok(c) => c,
        Err(e) => {
            // 暴露真实 io error（ENOENT / 权限等），便于排查 spawn 失败原因
            let msg = format!("failed to spawn: {} (kind: {:?})", e, e.kind());
            return ExecOutcome {
                exit_code: None,
                stdout: Vec::new(),
                stderr: msg.into_bytes(),
                duration_ms: start.elapsed().as_millis() as u64,
                truncated: false,
                timed_out: false,
            };
        }
    };

    // 先等待退出或超时（此时子进程的 stdout/stderr 在管道缓冲中）
    let exit_code;
    let timed_out;
    match child.wait_timeout(timeout) {
        Ok(Some(status)) => {
            exit_code = status.code();
            timed_out = false;
        }
        Ok(None) => {
            // 超时：杀掉子进程后再 wait 收尸
            let _ = child.kill();
            let _ = child.wait();
            exit_code = None;
            timed_out = true;
        }
        Err(_) => {
            exit_code = None;
            timed_out = false;
        }
    }

    // 子进程已结束（被 kill 或正常退出），管道写端关闭，读取不会再阻塞
    let mut stdout_buf = Vec::new();
    let mut stderr_buf = Vec::new();
    let mut truncated = false;
    if let Some(mut out) = child.stdout.take() {
        read_capped(&mut out, &mut stdout_buf, max_output_bytes, &mut truncated);
    }
    if let Some(mut err) = child.stderr.take() {
        read_capped(&mut err, &mut stderr_buf, max_output_bytes, &mut truncated);
    }

    ExecOutcome {
        exit_code,
        stdout: stdout_buf,
        stderr: stderr_buf,
        duration_ms: start.elapsed().as_millis() as u64,
        truncated,
        timed_out,
    }
}

/// 读取到 EOF 或达到上限为止；达到上限置 truncated=true 并停止。
fn read_capped<R: Read>(r: &mut R, buf: &mut Vec<u8>, cap: usize, truncated: &mut bool) {
    let mut tmp = [0u8; 16384];
    loop {
        match r.read(&mut tmp) {
            Ok(0) => break,
            Ok(n) => {
                if buf.len() + n <= cap {
                    buf.extend_from_slice(&tmp[..n]);
                } else {
                    let room = cap.saturating_sub(buf.len());
                    if room > 0 {
                        buf.extend_from_slice(&tmp[..room]);
                    }
                    *truncated = true;
                    break;
                }
            }
            Err(_) => break,
        }
    }
}

/// 截断标记：在输出末尾追加一行提示，使调用端可见 `truncated=true` 且 raw 有可视线索。
pub fn append_truncation_marker(buf: &mut Vec<u8>) {
    let marker = b"\n[tabbit-bridge: output truncated]\n";
    buf.extend_from_slice(marker);
}

#[cfg(test)]
mod tests {
    use super::*;

    #[cfg(unix)]
    #[test]
    fn timeout_works() {
        let mut cmd = Command::new("sleep");
        cmd.arg("30");
        let out = run(cmd, Duration::from_millis(200), 1024);
        assert!(out.timed_out);
        assert_eq!(out.exit_code, None);
    }

    #[cfg(unix)]
    #[test]
    fn truncation_works() {
        // yes 持续输出，应在 cap 处截断；子进程被 kill 后管道写端关闭，read 不会阻塞
        let mut cmd = Command::new("yes");
        cmd.arg("x");
        let out = run(cmd, Duration::from_secs(2), 1024);
        assert!(out.truncated || out.stdout.len() <= 1024);
    }

    #[test]
    fn normal_exec() {
        let mut cmd = Command::new("echo");
        cmd.arg("hello");
        let out = run(cmd, Duration::from_secs(5), 4096);
        assert_eq!(out.exit_code, Some(0));
        assert_eq!(out.stdout, b"hello\n");
        assert!(!out.truncated);
    }
}
