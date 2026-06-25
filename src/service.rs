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

/// 获取当前用户的 launchd GUI domain target（如 "gui/501"）。
#[cfg(target_os = "macos")]
fn gui_domain() -> String {
    // 通过 id -u 获取 UID；失败时回退到 501（macOS 默认首用户）。
    let uid = Command::new("id")
        .arg("-u")
        .output()
        .ok()
        .and_then(|o| String::from_utf8(o.stdout).ok())
        .map(|s| s.trim().to_string())
        .unwrap_or_else(|| "501".to_string());
    format!("gui/{uid}")
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
    fs::create_dir_all(&cfg_dir)?;
    let cfg_path = cfg_dir.join("config.toml").display().to_string();
    let log_out = cfg_dir.join("tabbit-bridge.out.log").display().to_string();
    let log_err = cfg_dir.join("tabbit-bridge.err.log").display().to_string();
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
    <key>StandardOutPath</key><string>{log_out}</string>
    <key>StandardErrorPath</key><string>{log_err}</string>
    <key>ProcessType</key><string>Background</string>
</dict>
</plist>
"#
    );
    fs::write(&plist, xml)?;

    // 新 API 对 plist 权限更严格：必须 644 且 owner 为当前用户。
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let mut perms = fs::metadata(&plist)?.permissions();
        perms.set_mode(0o644);
        fs::set_permissions(&plist, perms)?;
    }

    let plist_str = plist.display().to_string();
    let domain = gui_domain();
    let target = format!("{domain}/{LABEL}");

    // 幂等：先 bootout（忽略错误，可能本来就没加载）
    let _ = Command::new("launchctl")
        .args(["bootout", &target])
        .output();

    // bootstrap 加载
    let status = Command::new("launchctl")
        .args(["bootstrap", &domain, &plist_str])
        .status()?;
    if !status.success() {
        // fallback：极老系统不支持 bootstrap，退回旧 API
        eprintln!("[tabbit-bridge] bootstrap 失败，尝试 legacy launchctl load...");
        let legacy = Command::new("launchctl")
            .args(["load", "-w", &plist_str])
            .status()?;
        if !legacy.success() {
            return Err(std::io::Error::new(
                std::io::ErrorKind::Other,
                "launchctl bootstrap 与 legacy load 均失败",
            ));
        }
    }

    // 显式 enable（bootstrap 一般已 enable，但兜底）
    let _ = Command::new("launchctl")
        .args(["enable", &target])
        .status();

    eprintln!("[tabbit-bridge] 已注册 launchd 服务: {}", plist.display());
    Ok(())
}

#[cfg(target_os = "macos")]
pub fn uninstall() -> std::io::Result<()> {
    let Some(plist) = launchd_plist_path() else {
        return Ok(());
    };
    let domain = gui_domain();
    let target = format!("{domain}/{LABEL}");

    // 新 API：bootout 接收 service target 而非 plist 路径
    let _ = Command::new("launchctl")
        .args(["bootout", &target])
        .status();
    // legacy fallback：在 bootout 不可用的极老系统上仍能卸载
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
        "[Unit]
Description=Tabbit Bridge
After=network.target

[Service]
ExecStart={exe_str} --config-dir {cfg_path}
Restart=on-failure

[Install]
WantedBy=default.target
"
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
    use std::ffi::OsString;
    use windows_service::service::{
        ServiceAccess, ServiceErrorControl, ServiceInfo, ServiceStartType, ServiceType,
    };
    use windows_service::service_manager::{ServiceManager, ServiceManagerAccess};

    let manager_access = ServiceManagerAccess::CONNECT | ServiceManagerAccess::CREATE_SERVICE;
    let scm = ServiceManager::local_computer(None::<&str>, manager_access)
        .map_err(|e| std::io::Error::new(std::io::ErrorKind::Other, format!("SCM: {e}")))?;

    let exe = current_exe();
    let svc_info = ServiceInfo {
        name: OsString::from("tabbit-bridge"),
        display_name: OsString::from("Tabbit Bridge"),
        service_type: ServiceType::OWN_PROCESS,
        start_type: ServiceStartType::AutoStart,
        error_control: ServiceErrorControl::Normal,
        executable_path: exe,
        launch_arguments: vec![OsString::from("--service")],
        dependencies: vec![],
        account_name: None,
        account_password: None,
    };
    let service = scm
        .create_service(&svc_info, ServiceAccess::START)
        .map_err(|e| std::io::Error::new(std::io::ErrorKind::Other, format!("create: {e}")))?;
    let _ = service.start::<&str>(&[]);
    eprintln!("[tabbit-bridge] 已注册 Windows 服务");
    Ok(())
}

#[cfg(windows)]
pub fn uninstall() -> std::io::Result<()> {
    use windows_service::service::ServiceAccess;
    use windows_service::service_manager::{ServiceManager, ServiceManagerAccess};

    let scm = ServiceManager::local_computer(None::<&str>, ServiceManagerAccess::CONNECT)
        .map_err(|e| std::io::Error::new(std::io::ErrorKind::Other, format!("SCM: {e}")))?;
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
