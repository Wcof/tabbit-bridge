//! 配置自举：首次启动随机生成端口与 256-bit token，以 0600 写入 config.toml。
//! 已存在则加载并校验。全程不把 token 写入日志。

use std::fs;
use std::io::Write;
use std::path::{Path, PathBuf};

use getrandom::getrandom;
use serde::{Deserialize, Serialize};

const TOKEN_BYTES: usize = 32; // 256-bit

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Config {
    pub server: ServerCfg,
    #[serde(default)]
    pub limits: LimitsCfg,
    #[serde(default)]
    pub update: UpdateCfg,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ServerCfg {
    pub bind: String,
    pub port: u16,
    pub token: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct LimitsCfg {
    #[serde(default = "default_timeout")]
    pub timeout_ms: u64,
    #[serde(default = "default_max_output")]
    pub max_output_bytes: usize,
    #[serde(default = "default_rate")]
    pub rate_per_min: u32,
}

fn default_timeout() -> u64 {
    5000
}
fn default_max_output() -> usize {
    2_000_000
}
fn default_rate() -> u32 {
    60
}

impl Default for LimitsCfg {
    fn default() -> Self {
        Self {
            timeout_ms: default_timeout(),
            max_output_bytes: default_max_output(),
            rate_per_min: default_rate(),
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct UpdateCfg {
    #[serde(default = "default_check_on_start")]
    pub check_on_start: bool,
    #[serde(default)]
    pub auto_install: bool,
    #[serde(default = "default_channel")]
    pub channel: String,
    #[serde(default)]
    pub last_check_ts: i64,
    #[serde(default)]
    pub last_known_version: String,
}

fn default_check_on_start() -> bool {
    true
}
fn default_channel() -> String {
    "stable".to_string()
}

/// 返回当前平台的配置目录（不创建）。
pub fn config_dir() -> Option<PathBuf> {
    if let Ok(d) = std::env::var("TABBIT_BRIDGE_CONFIG_DIR") {
        if !d.is_empty() {
            return Some(PathBuf::from(d));
        }
    }
    #[cfg(target_os = "macos")]
    {
        if let Ok(home) = std::env::var("HOME") {
            return Some(PathBuf::from(home).join("Library/Application Support/tabbit-bridge"));
        }
    }
    #[cfg(all(unix, not(target_os = "macos")))]
    {
        if let Ok(home) = std::env::var("HOME") {
            return Some(PathBuf::from(home).join(".config/tabbit-bridge"));
        }
    }
    #[cfg(windows)]
    {
        if let Ok(appdata) = std::env::var("APPDATA") {
            return Some(PathBuf::from(appdata).join("tabbit-bridge"));
        }
    }
    None
}

fn random_hex(n: usize) -> String {
    let mut buf = vec![0u8; n];
    // getrandom 在足够新的版本下 infallible；这里保留 Result 处理以兼容旧版。
    getrandom(&mut buf).expect("getrandom failed");
    let mut s = String::with_capacity(n * 2);
    for b in &buf {
        s.push_str(&format!("{:02x}", b));
    }
    s
}

/// 在 [40000, 60000] 范围内随机取一个高位端口。
fn random_high_port() -> u16 {
    let mut b = [0u8; 2];
    getrandom(&mut b).expect("getrandom failed");
    let v = u16::from_be_bytes(b);
    40000 + (v % 20001)
}

/// 原子创建并写入密钥文件。用 `create_new` 保证并发只有一个进程成功，
/// 失败方会拿到 `AlreadyExists`，由调用方回退到读分支。
fn write_secret(path: &Path, content: &str) -> std::io::Result<()> {
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent)?;
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let mut perms = fs::metadata(parent)?.permissions();
            perms.set_mode(0o700);
            fs::set_permissions(parent, perms)?;
        }
    }
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        let mut f = fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .mode(0o600)
            .open(path)?;
        f.write_all(content.as_bytes())?;
    }
    #[cfg(not(unix))]
    {
        let mut f = fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(path)?;
        f.write_all(content.as_bytes())?;
    }
    Ok(())
}

/// 加载或自举配置。返回 (Config, config_path)。
/// 注意：禁止将 token 写入日志，本函数仅在调用方显式需要时返回 token。
pub fn load_or_init() -> std::io::Result<(Config, PathBuf)> {
    let dir = config_dir().ok_or_else(|| {
        std::io::Error::new(std::io::ErrorKind::NotFound, "无法确定配置目录")
    })?;
    let path = dir.join("config.toml");

    if path.exists() {
        let text = fs::read_to_string(&path)?;
        let cfg: Config = toml::from_str(&text).map_err(|e| {
            std::io::Error::new(std::io::ErrorKind::InvalidData, format!("config.toml 解析失败: {}", e))
        })?;
        // 基本校验
        if cfg.server.token.len() != TOKEN_BYTES * 2 {
            return Err(std::io::Error::new(
                std::io::ErrorKind::InvalidData,
                "config.toml 中 token 长度非法，应为 64 个 hex 字符",
            ));
        }
        if cfg.server.bind != "127.0.0.1" {
            return Err(std::io::Error::new(
                std::io::ErrorKind::InvalidData,
                "安全约束：server.bind 必须为 127.0.0.1",
            ));
        }
        return Ok((cfg, path));
    }

    // 自举：用 create_new 原子创建，并发时只有一个进程成功。
    // 失败方（拿到 AlreadyExists）回退到读分支，加载对方刚写好的配置。
    let cfg = Config {
        server: ServerCfg {
            bind: "127.0.0.1".to_string(),
            port: random_high_port(),
            token: random_hex(TOKEN_BYTES),
        },
        limits: LimitsCfg::default(),
        update: UpdateCfg::default(),
    };
    let text = toml::to_string(&cfg).map_err(|e| {
        std::io::Error::new(std::io::ErrorKind::InvalidData, format!("序列化配置失败: {}", e))
    })?;
    match write_secret(&path, &text) {
        Ok(()) => {
            eprintln!("[tabbit-bridge] 首次启动，已生成配置: {}", path.display());
            eprintln!("[tabbit-bridge] 监听端口: {}", cfg.server.port);
            Ok((cfg, path))
        }
        Err(e) if e.kind() == std::io::ErrorKind::AlreadyExists => {
            // 并发自举：另一进程已抢先创建，回退读分支
            let text = fs::read_to_string(&path)?;
            let cfg: Config = toml::from_str(&text).map_err(|e| {
                std::io::Error::new(std::io::ErrorKind::InvalidData, format!("config.toml 解析失败: {}", e))
            })?;
            if cfg.server.token.len() != TOKEN_BYTES * 2 {
                return Err(std::io::Error::new(
                    std::io::ErrorKind::InvalidData,
                    "config.toml 中 token 长度非法，应为 64 个 hex 字符",
                ));
            }
            if cfg.server.bind != "127.0.0.1" {
                return Err(std::io::Error::new(
                    std::io::ErrorKind::InvalidData,
                    "安全约束：server.bind 必须为 127.0.0.1",
                ));
            }
            Ok((cfg, path))
        }
        Err(e) => Err(e),
    }
}

/// 把后台检查到的最新版本号写回 config.toml 的 [update] 段。
/// 失败静默（不影响主服务）。时间戳用系统 epoch 秒。
pub fn record_latest(version: &str) -> std::io::Result<()> {
    let dir = config_dir().ok_or_else(|| {
        std::io::Error::new(std::io::ErrorKind::NotFound, "无法确定配置目录")
    })?;
    let path = dir.join("config.toml");
    let text = fs::read_to_string(&path)?;
    let mut cfg: Config = toml::from_str(&text).map_err(|e| {
        std::io::Error::new(std::io::ErrorKind::InvalidData, format!("config.toml 解析失败: {}", e))
    })?;
    cfg.update.last_known_version = version.to_string();
    cfg.update.last_check_ts = now_epoch_secs();
    let new_text = toml::to_string(&cfg).map_err(|e| {
        std::io::Error::new(std::io::ErrorKind::InvalidData, format!("序列化配置失败: {}", e))
    })?;
    // 已有文件，普通覆写即可（自举竞争已由 create_new 解决）
    fs::write(&path, new_text)?;
    Ok(())
}

fn now_epoch_secs() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs() as i64)
        .unwrap_or(0)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn random_hex_is_64_chars() {
        let s = random_hex(32);
        assert_eq!(s.len(), 64);
        assert!(s.chars().all(|c| c.is_ascii_hexdigit()));
    }

    #[test]
    fn random_high_port_in_range() {
        for _ in 0..100 {
            let p = random_high_port();
            assert!((40000..=60000).contains(&p));
        }
    }
}
