//! 入口：加载/自举配置，根据 CLI 参数决定运行模式。
//! Windows 上消除黑窗（仅在二进制顶层属性生效，不影响 cargo test）。

#![cfg_attr(all(windows, not(test)), windows_subsystem = "windows")]

mod config;
mod exec;
mod registry;
mod server;
mod service;
mod updater;

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
            "--check-update" => mode = Mode::CheckUpdate,
            "--self-update" => mode = Mode::SelfUpdate,
            "--service" => mode = Mode::Run, // Windows SCM 启动标志，等同 Run
            "-h" | "--help" => {
                print_help();
                exit(0);
            }
            _ => {}
        }
        i += 1;
    }

    // --check-update 和 --self-update 不依赖配置，跳过 load_or_init 避免意外自举
    match mode {
        Mode::CheckUpdate => {
            match updater::check_latest() {
                Ok(r) if updater::is_newer(&r.version, updater::current_version()) => {
                    println!("update_available={}", r.version);
                    exit(0);
                }
                Ok(_) => {
                    println!("up_to_date={}", updater::current_version());
                    exit(0);
                }
                Err(e) => {
                    eprintln!("check failed: {e}");
                    exit(2);
                }
            }
        }
        Mode::SelfUpdate => {
            // 由 tb upgrade 调用，子进程模式：仅下载与替换，不重启（重启由 tb 控制）
            let r = match updater::check_latest() {
                Ok(r) => r,
                Err(e) => {
                    eprintln!("check failed: {e}");
                    exit(2);
                }
            };
            if !updater::is_newer(&r.version, updater::current_version()) {
                println!("already_latest");
                exit(0);
            }
            let triple = updater::detect_target_triple();
            let new_bin = match updater::download(&r.tag, triple) {
                Ok(p) => p,
                Err(e) => {
                    eprintln!("download failed: {e}");
                    exit(1);
                }
            };
            let cur = match std::env::current_exe() {
                Ok(p) => p,
                Err(e) => {
                    eprintln!("current_exe failed: {e}");
                    exit(1);
                }
            };
            if let Err(e) = updater::apply(&new_bin, &cur) {
                eprintln!("apply failed: {e}");
                exit(1);
            }
            println!("updated_to={}", r.version);
            return;
        }
        _ => {}
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
        Mode::CheckUpdate | Mode::SelfUpdate => unreachable!(), // 已在上方提前返回
    }
}

enum Mode {
    Run,
    Install,
    Uninstall,
    PrintToken,
    CheckUpdate,
    SelfUpdate,
}

fn print_help() {
    eprintln!("tabbit-bridge v{}", env!("CARGO_PKG_VERSION"));
    eprintln!("用法:");
    eprintln!("  tabbit-bridge                       以加载的配置启动 HTTP 服务");
    eprintln!("  tabbit-bridge --install             注册为后台守护进程");
    eprintln!("  tabbit-bridge --uninstall           注销后台守护进程");
    eprintln!("  tabbit-bridge --print-token         打印当前 token（供填入妙招脚本）");
    eprintln!("  tabbit-bridge --check-update        检查 GitHub Releases 最新稳定版");
    eprintln!("  tabbit-bridge --self-update         下载并原子替换当前二进制（由 tb upgrade 调用）");
    eprintln!("  tabbit-bridge --config-dir <dir>    指定配置目录");
    eprintln!("  tabbit-bridge --service             Windows SCM 调用入口");
}
