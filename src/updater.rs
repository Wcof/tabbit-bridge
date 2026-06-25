//! 自动更新：检查 GitHub Releases、下载稳定版、原子替换。
//! 不引入 HTTP 库，复用系统 curl（macOS/Linux 自带，Windows 10+ 自带）。

use std::path::{Path, PathBuf};
use std::process::Command;

const REPO: &str = "Wcof/tabbit-bridge";
const CURRENT: &str = env!("CARGO_PKG_VERSION");

pub struct ReleaseInfo {
    pub tag: String,      // "v1.2.1"
    pub version: String,  // "1.2.1"
}

/// 查询 GitHub 最新稳定版（API /releases/latest 自动过滤 prerelease / draft）。
pub fn check_latest() -> std::io::Result<ReleaseInfo> {
    let url = format!("https://api.github.com/repos/{REPO}/releases/latest");
    let out = Command::new("curl")
        .args([
            "-fsSL",
            "-H",
            "Accept: application/vnd.github+json",
            "--max-time",
            "10",
            &url,
        ])
        .output()?;
    if !out.status.success() {
        return Err(std::io::Error::new(std::io::ErrorKind::Other, "curl 查询失败"));
    }
    let json: serde_json::Value = serde_json::from_slice(&out.stdout)
        .map_err(|e| std::io::Error::new(std::io::ErrorKind::InvalidData, e))?;
    let tag = json
        .get("tag_name")
        .and_then(|v| v.as_str())
        .ok_or_else(|| std::io::Error::new(std::io::ErrorKind::NotFound, "无 tag_name"))?
        .to_string();
    let version = tag.trim_start_matches('v').to_string();
    Ok(ReleaseInfo { tag, version })
}

/// 比较版本（仅支持 x.y.z 数字三段）。
pub fn is_newer(latest: &str, current: &str) -> bool {
    let parse = |s: &str| -> (u32, u32, u32) {
        let mut it = s.split('.').map(|x| x.parse::<u32>().unwrap_or(0));
        (
            it.next().unwrap_or(0),
            it.next().unwrap_or(0),
            it.next().unwrap_or(0),
        )
    };
    parse(latest) > parse(current)
}

/// 编译期决定当前二进制的 target triple。
pub fn detect_target_triple() -> &'static str {
    let arch = if cfg!(target_arch = "aarch64") {
        "aarch64"
    } else if cfg!(target_arch = "x86_64") {
        "x86_64"
    } else {
        "unknown"
    };
    let os = if cfg!(target_os = "macos") {
        "apple-darwin"
    } else if cfg!(target_os = "linux") {
        "unknown-linux-musl"
    } else if cfg!(target_os = "windows") {
        "pc-windows-msvc"
    } else {
        "unknown"
    };
    // 用运行时拼接避免 const fn 限制；编译期 cfg 已锁定。
    // 由于返回 &'static str，这里用静态分发的方式：常见三元先列举。
    match (arch, os) {
        ("aarch64", "apple-darwin") => "aarch64-apple-darwin",
        ("x86_64", "apple-darwin") => "x86_64-apple-darwin",
        ("x86_64", "unknown-linux-musl") => "x86_64-unknown-linux-musl",
        ("aarch64", "unknown-linux-musl") => "aarch64-unknown-linux-musl",
        ("x86_64", "pc-windows-msvc") => "x86_64-pc-windows-msvc",
        _ => "unknown-unknown",
    }
}

/// 下载并校验 SHA256，解压后返回二进制临时路径。
pub fn download(tag: &str, target_triple: &str) -> std::io::Result<PathBuf> {
    let tmp = std::env::temp_dir().join(format!("tabbit-bridge-{tag}"));
    std::fs::remove_dir_all(&tmp).ok();
    std::fs::create_dir_all(&tmp)?;
    let archive_ext = if cfg!(windows) { "zip" } else { "tar.gz" };
    let asset = format!("tabbit-bridge-{target_triple}.{archive_ext}");
    let base = format!("https://github.com/{REPO}/releases/download/{tag}");

    let archive_path = tmp.join(&asset);
    let sha_path = tmp.join(format!("{asset}.sha256"));

    run_curl(&format!("{base}/{asset}"), &archive_path)?;
    run_curl(&format!("{base}/{asset}.sha256"), &sha_path)?;

    verify_sha256(&archive_path, &sha_path)?;
    extract(&archive_path, &tmp)?;

    let bin_name = if cfg!(windows) {
        "tabbit-bridge.exe"
    } else {
        "tabbit-bridge"
    };
    Ok(tmp.join(bin_name))
}

fn run_curl(url: &str, out: &Path) -> std::io::Result<()> {
    let s = Command::new("curl")
        .args(["-fsSL", "--max-time", "60", "-o"])
        .arg(out)
        .arg(url)
        .status()?;
    if !s.success() {
        return Err(std::io::Error::new(std::io::ErrorKind::Other, "下载失败"));
    }
    Ok(())
}

fn verify_sha256(file: &Path, sha: &Path) -> std::io::Result<()> {
    let expected = std::fs::read_to_string(sha)?;
    let expected = expected
        .split_whitespace()
        .next()
        .unwrap_or("")
        .to_lowercase();
    if expected.len() != 64 {
        return Err(std::io::Error::new(
            std::io::ErrorKind::InvalidData,
            "sha256 文件格式异常",
        ));
    }
    // 复用系统工具校验，避免引入 sha2 crate
    let (tool, args): (&str, &[&str]) = if cfg!(target_os = "macos") {
        ("shasum", &["-a", "256"])
    } else {
        ("sha256sum", &[])
    };
    let out = Command::new(tool).args(args).arg(file).output()?;
    if !out.status.success() {
        return Err(std::io::Error::new(
            std::io::ErrorKind::Other,
            "sha256 工具执行失败",
        ));
    }
    let got = String::from_utf8_lossy(&out.stdout);
    let got = got.split_whitespace().next().unwrap_or("").to_lowercase();
    if got != expected {
        return Err(std::io::Error::new(
            std::io::ErrorKind::InvalidData,
            "SHA256 校验失败",
        ));
    }
    Ok(())
}

fn extract(archive: &Path, dest: &Path) -> std::io::Result<()> {
    let s = if archive.extension().and_then(|s| s.to_str()) == Some("zip") {
        Command::new("unzip")
            .arg("-o")
            .arg(archive)
            .arg("-d")
            .arg(dest)
            .status()?
    } else {
        Command::new("tar")
            .args(["-xzf"])
            .arg(archive)
            .arg("-C")
            .arg(dest)
            .status()?
    };
    if !s.success() {
        return Err(std::io::Error::new(std::io::ErrorKind::Other, "解压失败"));
    }
    Ok(())
}

/// 原子替换：备份旧 → rename 新 → 失败回滚。
pub fn apply(new_bin: &Path, current_bin: &Path) -> std::io::Result<()> {
    let backup = current_bin.with_extension("bak");
    let _ = std::fs::remove_file(&backup);
    std::fs::rename(current_bin, &backup)?;
    if let Err(e) = std::fs::rename(new_bin, current_bin) {
        // 回滚
        let _ = std::fs::rename(&backup, current_bin);
        return Err(e);
    }
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let mut p = std::fs::metadata(current_bin)?.permissions();
        p.set_mode(0o755);
        std::fs::set_permissions(current_bin, p)?;
    }
    Ok(())
}

pub fn current_version() -> &'static str {
    CURRENT
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn version_compare_basic() {
        assert!(is_newer("1.2.1", "1.2.0"));
        assert!(is_newer("2.0.0", "1.9.9"));
        assert!(!is_newer("1.2.0", "1.2.0"));
        assert!(!is_newer("1.1.9", "1.2.0"));
    }

    #[test]
    fn detect_triple_non_unknown_on_supported() {
        // 至少在 CI 已知平台上不能落到 unknown
        let t = detect_target_triple();
        assert!(!t.starts_with("unknown-"), "当前平台 triple 解析失败: {t}");
    }
}
