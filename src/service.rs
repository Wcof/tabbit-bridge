//! 三平台守护进程注册/注销。
//! - macOS: ~/Library/LaunchAgents/com.tabbit.bridge.plist + launchctl load
//! - Linux: ~/.config/systemd/user/tabbit-bridge.service + systemctl --user enable --now
//! - Windows: windows-service 注册为后台服务
//!
//! Windows 编译加 `#![windows_subsystem = "windows"]` 杜绝黑窗（见 main.rs 顶部属性）。

use std::fs;
use std::path::PathBuf;
use std::process::Command;

use crate::config;

const LABEL: &str = "com.tabbit.bridge";

fn current_exe() -> PathBuf {
    std::env::current_exe().unwrap_or_else(|_| PathBuf::from("tabbit-bridge"))
}

#[cfg(target_os = "macos")]
fn launchd_plist_path() -> Option<PathBuf> {
    let home = std::env::var("HOME").ok()?;
    Some(
        PathBuf::from(home)
            .join("Library/LaunchAgents")
            .join(format!("{LABEL}.plist")),
    )
}

#[cfg(target_os = "macos")]
pub fn install() -> std::io::Result<()> {
    let plist = launchd_plist_path()
        .ok_or_else(|| std::io::Error::new(std::io::ErrorKind::NotFound, "HOME 未设置"))?;
    if let Some(p) = plist.parent() {
        fs::create_dir_all(p)?;
    }
    let exe = current_exe();
    let exe_str = exe.display();
    let cfg_dir = config::config_dir()
        .ok_or_else(|| std::io::Error::new(std::io::ErrorKind::NotFound, "无配置目录"))?;
    let cfg_path = cfg_dir.join("config.toml").display().to_string();
    let xml = format!(
        r#"<?xml version="1.0" encoding="UTF-8"?>
<!DOCTYPE plist PUBLIC "-//Apple//DTD PLIST 1.0//EN" "http://www.apple.com/DTDs/PropertyList-1.0.dtd">
<plist version="1.0">
<dict>
    <key>Label</key><string>{LABEL}</string>
    <key>ProgramArguments</key>
    <array>
        <string>{exe_str}</string>
        <string>--config-dir</string>
        <string>{cfg_path}</string>
    </array>
    <key>RunAtLoad</key><true/>
    <key>KeepAlive</key><true/>
    <key>StandardOutPath</key><string>/tmp/tabbit-bridge.out.log</string>
    <key>StandardErrorPath</key><string>/tmp/tabbit-bridge.err.log</string>
    <key>ProcessType</key><string>Background</string>
</dict>
</plist>
"#
    );
    fs::write(&plist, xml)?;
    let out = Command::new("launchctl")
        .args(["unload", &plist.display().to_string()])
        .output()
        .ok();
    let _ = out;
    let status = Command::new("launchctl")
        .args(["load", &plist.display().to_string()])
        .status()?;
    if !status.success() {
        return Err(std::io::Error::new(
            std::io::ErrorKind::Other,
            "launchctl load 失败",
        ));
    }
    eprintln!("[tabbit-bridge] 已注册 launchd 服务: {}", plist.display());
    Ok(())
}

#[cfg(target_os = "macos")]
pub fn uninstall() -> std::io::Result<()> {
    let Some(plist) = launchd_plist_path() else {
        return Ok(());
    };
    let _ = Command::new("launchctl")
        .args(["unload", &plist.display().to_string()])
        .status();
    let _ = fs::remove_file(&plist);
    eprintln!("[tabbit-bridge] 已注销 launchd 服务");
    Ok(())
}

#[cfg(all(unix, not(target_os = "macos")))]
fn systemd_unit_path() -> Option<PathBuf> {
    let home = std::env::var("HOME").ok()?;
    Some(
        PathBuf::from(home)
            .join(".config/systemd/user")
            .join("tabbit-bridge.service"),
    )
}

#[cfg(all(unix, not(target_os = "macos")))]
pub fn install() -> std::io::Result<()> {
    let unit = systemd_unit_path()
        .ok_or_else(|| std::io::Error::new(std::io::ErrorKind::NotFound, "HOME 未设置"))?;
    if let Some(p) = unit.parent() {
        fs::create_dir_all(p)?;
    }
    let exe = current_exe();
    let exe_str = exe.display();
    let cfg_dir = config::config_dir()
        .ok_or_else(|| std::io::Error::new(std::io::ErrorKind::NotFound, "无配置目录"))?;
    let cfg_path = cfg_dir.join("config.toml").display().to_string();
    let unit_text = format!(
        "[Unit]\nDescription=Tabbit Bridge\nAfter=network.target\n\n\
[Service]\nExecStart={exe_str} --config-dir {cfg_path}\nRestart=on-failure\n\n\
[Install]\nWantedBy=default.target\n"
    );
    fs::write(&unit, unit_text)?;
    let s = Command::new("systemctl")
        .args(["--user", "daemon-reload"])
        .status()?;
    if !s.success() {
        return Err(std::io::Error::new(std::io::ErrorKind::Other, "daemon-reload 失败"));
    }
    let s = Command::new("systemctl")
        .args(["--user", "enable", "--now", "tabbit-bridge.service"])
        .status()?;
    if !s.success() {
        return Err(std::io::Error::new(std::io::ErrorKind::Other, "enable --now 失败"));
    }
    eprintln!("[tabbit-bridge] 已注册 systemd-user 服务: {}", unit.display());
    Ok(())
}

#[cfg(all(unix, not(target_os = "macos")))]
pub fn uninstall() -> std::io::Result<()> {
    let _ = Command::new("systemctl")
        .args(["--user", "disable", "--now", "tabbit-bridge.service"])
        .status();
    let Some(unit) = systemd_unit_path() else {
        return Ok(());
    };
    let _ = fs::remove_file(&unit);
    let _ = Command::new("systemctl").args(["--user", "daemon-reload"]).status();
    eprintln!("[tabbit-bridge] 已注销 systemd-user 服务");
    Ok(())
}

#[cfg(windows)]
pub fn install() -> std::io::Result<()> {
    use windows_service::service::{
        ServiceAccess, ServiceErrorControl, ServiceInfo, ServiceStartType,
        ServiceState, ServiceType,
    };
    use windows_service::service_manager::{ServiceAccess, ServiceManager};

    let manager = ServiceManager::local_computer(None::<&str>)
        .map_err(|e| std::io::Error::new(std::io::ErrorKind::Other, format!(" SCM: {e}")))?;
    let scm = manager
        .access(ServiceAccess::CONNECT | ServiceAccess::CREATE_SERVICE)
        .map_err(|e| std::io::Error::new(std::io::ErrorKind::Other, format!(" SCM access: {e}")))?;
    let exe = current_exe();
    let svc_info = ServiceInfo {
        name: "tabbit-bridge".into(),
        display_name: "Tabbit Bridge".into(),
        service_type: ServiceType::OwnProcess,
        start_type: ServiceStartType::AutoStart,
        error_control: ServiceErrorControl::Normal,
        executable_path: exe,
        launch_arguments: vec!["--service".into()],
        dependencies: vec![],
        description: None,
    };
    scm.create_service(&svc_info, ServiceAccess::START)
        .map_err(|e| std::io::Error::new(std::io::ErrorKind::Other, format!("create: {e}")))?;
    eprintln!("[tabbit-bridge] 已注册 Windows 服务");
    Ok(())
}

#[cfg(windows)]
pub fn uninstall() -> std::io::Result<()> {
    use windows_service::service::{ServiceAccess, ServiceState};
    use windows_service::service_manager::{ServiceAccess, ServiceManager};

    let manager = ServiceManager::local_computer(None::<&str>)
        .map_err(|e| std::io::Error::new(std::io::ErrorKind::Other, format!(" SCM: {e}")))?;
    let scm = manager
        .access(ServiceAccess::CONNECT | ServiceAccess::DELETE)
        .map_err(|e| std::io::Error::new(std::io::ErrorKind::Other, format!(" SCM access: {e}")))?;
    let svc = scm
        .open_service("tabbit-bridge", ServiceAccess::DELETE | ServiceAccess::STOP)
        .map_err(|e| std::io::Error::new(std::io::ErrorKind::Other, format!("open: {e}")))?;
    let _ = svc.stop();
    svc.delete()
        .map_err(|e| std::io::Error::new(std::io::ErrorKind::Other, format!("delete: {e}")))?;
    eprintln!("[tabbit-bridge] 已注销 Windows 服务");
    Ok(())
}

#[cfg(not(any(unix, windows)))]
pub fn install() -> std::io::Result<()> {
    Err(std::io::Error::new(
        std::io::ErrorKind::Unsupported,
        "当前平台不支持守护注册",
    ))
}

#[cfg(not(any(unix, windows)))]
pub fn uninstall() -> std::io::Result<()> {
    Err(std::io::Error::new(
        std::io::ErrorKind::Unsupported,
        "当前平台不支持守护注销",
    ))
}
