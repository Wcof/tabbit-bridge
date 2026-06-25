//! 入口：加载/自举配置，根据 CLI 参数决定运行模式。
//! Windows 上消除黑窗（仅在二进制顶层属性生效，不影响 cargo test）。

#![cfg_attr(all(windows, not(test)), windows_subsystem = "windows")]

mod config;
mod exec;
mod registry;
mod server;
mod service;

use std::process::exit;

fn main() {
    let args: Vec<String> = std::env::args().collect();
    let mut config_dir_override: Option<String> = None;
    let mut mode = Mode::Run;

    let mut i = 1;
    while i < args.len() {
        match args[i].as_str() {
            "--config-dir" => {
                i += 1;
                if i < args.len() {
                    config_dir_override = Some(args[i].clone());
                }
            }
            "--install" => mode = Mode::Install,
            "--uninstall" => mode = Mode::Uninstall,
            "--print-token" => mode = Mode::PrintToken,
            "--service" => mode = Mode::Run, // Windows SCM 启动标志，等同 Run
            "-h" | "--help" => {
                print_help();
                exit(0);
            }
            _ => {}
        }
        i += 1;
    }

    if let Some(d) = config_dir_override {
        // 仅设置 env，由 config::config_dir() 读取
        std::env::set_var("TABBIT_BRIDGE_CONFIG_DIR", d);
    }

    let (cfg, _path) = match config::load_or_init() {
        Ok(v) => v,
        Err(e) => {
            eprintln!("[tabbit-bridge] 配置加载失败: {e}");
            exit(1);
        }
    };

    match mode {
        Mode::Install => {
            if let Err(e) = service::install() {
                eprintln!("[tabbit-bridge] 安装守护失败: {e}");
                exit(1);
            }
        }
        Mode::Uninstall => {
            if let Err(e) = service::uninstall() {
                eprintln!("[tabbit-bridge] 注销守护失败: {e}");
                exit(1);
            }
        }
        Mode::PrintToken => {
            // 仅向 stdout 输出，便于安装脚本捕获；不进日志。
            println!("{}", cfg.server.token);
        }
        Mode::Run => {
            if let Err(e) = server::serve(&cfg) {
                eprintln!("[tabbit-bridge] 服务退出: {e}");
                exit(1);
            }
        }
    }
}

enum Mode {
    Run,
    Install,
    Uninstall,
    PrintToken,
}

fn print_help() {
    eprintln!("tabbit-bridge v{}", env!("CARGO_PKG_VERSION"));
    eprintln!("用法:");
    eprintln!("  tabbit-bridge                       以加载的配置启动 HTTP 服务");
    eprintln!("  tabbit-bridge --install             注册为后台守护进程");
    eprintln!("  tabbit-bridge --uninstall           注销后台守护进程");
    eprintln!("  tabbit-bridge --print-token         打印当前 token（供填入妙招脚本）");
    eprintln!("  tabbit-bridge --config-dir <dir>    指定配置目录");
    eprintln!("  tabbit-bridge --service             Windows SCM 调用入口");
}
