//! 编译期白名单：客户端只传 action id，真实程序与固定参数在此写死。
//! 受控参数模式：仅对显式开放的 action 接受经正则约束的值，作为独立 argv 元素传入。

use std::process::Command;

/// 白名单条目。
pub struct Entry {
    pub program: &'static str,
    pub args: &'static [&'static str],
    /// 若该 action 接受受控参数，则为 (字段名, 正则)；否则 None。
    pub controlled: Option<(&'static str, &'static str, &'static str)>,
    /// 是否将命令 stdout 当作 JSON 解析后放入 data 字段。
    pub parses_json: bool,
}

/// 解析 action id 为白名单条目。未命中返回 None（调用方返回 404）。
///
/// 注意：受控参数的校验仅做格式约束，绝不拼接 shell 字符串。
pub fn resolve(id: &str) -> Option<Entry> {
    match id {
        "rtk_gain" => Some(Entry {
            program: "rtk",
            args: &["gain", "--all", "--format", "json"],
            controlled: None,
            parses_json: true,
        }),
        "rtk_discover" => Some(Entry {
            program: "rtk",
            args: &["discover", "--all", "--since", "7"],
            controlled: None,
            parses_json: false,
        }),
        "cc_daily" => Some(Entry {
            program: "ccusage",
            args: &["daily", "--json"],
            controlled: None,
            parses_json: true,
        }),
        "cc_monthly" => Some(Entry {
            program: "ccusage",
            args: &["monthly", "--json"],
            controlled: None,
            parses_json: true,
        }),
        "cc_daily_at" => Some(Entry {
            program: "ccusage",
            args: &["daily", "--json"],
            controlled: Some(("since", "^[0-9]{4}-[0-9]{2}-[0-9]{2}$", "--since")),
            parses_json: true,
        }),
        _ => None,
    }
}

/// 简易正则：仅支持本白名单用到的「完全匹配 + 锚定 + 字符类」子集。
/// 为避免引入 regex crate（依赖约束），这里手写一个最小校验器：
/// 支持 `^...$` 锚定、`[0-9]` `[a-z]` 等字符类、`{n}` 重复、字面量。
/// 仅用于受控参数格式约束，不用于通用匹配。
pub fn matches_simple(pattern: &str, value: &str) -> bool {
    // 仅实现本仓库使用的两种模式：
    //   ^[0-9]{4}-[0-9]{2}-[0-9]{2}$
    // 如未来需要更多，应在此扩展或引入 regex（届时需更新依赖清单）。
    if pattern == "^[0-9]{4}-[0-9]{2}-[0-9]{2}$" {
        let bytes = value.as_bytes();
        if bytes.len() != 10 {
            return false;
        }
        let positions_digit = [0usize, 1, 2, 3, 5, 6, 8, 9];
        let positions_dash = [4usize, 7];
        for i in positions_digit {
            if !bytes[i].is_ascii_digit() {
                return false;
            }
        }
        for i in positions_dash {
            if bytes[i] != b'-' {
                return false;
            }
        }
        return true;
    }
    false
}

/// 构造已就绪的 Command（仅 new + args，绝不经过 shell）。
/// 受控参数通过 `controlled_value` 注入，作为独立 argv 元素。
pub fn build_command(entry: &Entry, controlled_value: Option<&str>) -> Option<Command> {
    let mut cmd = Command::new(entry.program);
    for a in entry.args {
        cmd.arg(a);
    }
    if let Some((_field, regex, flag)) = entry.controlled {
        let v = controlled_value?;
        if !matches_simple(regex, v) {
            return None;
        }
        cmd.arg(flag);
        cmd.arg(v);
    }
    Some(cmd)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn known_actions_resolve() {
        assert!(resolve("rtk_gain").is_some());
        assert!(resolve("cc_daily").is_some());
        assert!(resolve("rm_rf").is_none());
        assert!(resolve("").is_none());
    }

    #[test]
    fn date_format_validation() {
        assert!(matches_simple("^[0-9]{4}-[0-9]{2}-[0-9]{2}$", "2026-06-25"));
        assert!(!matches_simple("^[0-9]{4}-[0-9]{2}-[0-9]{2}$", "2026-6-25"));
        assert!(!matches_simple("^[0-9]{4}-[0-9]{2}-[0-9]{2}$", "abcd-ef-gh"));
        assert!(!matches_simple("^[0-9]{4}-[0-9]{2}-[0-9]{2}$", "2026-06-25; rm -rf /"));
    }

    #[test]
    fn controlled_rejects_bad_format() {
        let entry = resolve("cc_daily_at").unwrap();
        let cmd = build_command(&entry, Some("not-a-date"));
        assert!(cmd.is_none(), "非法格式的受控参数应被拒绝");
    }

    #[test]
    fn cc_daily_at_builds_single_since() {
        let entry = resolve("cc_daily_at").unwrap();
        let cmd = build_command(&entry, Some("2026-06-25")).unwrap();
        let args: Vec<_> = cmd.get_args().map(|s| s.to_string_lossy().to_string()).collect();
        assert_eq!(args, vec!["daily", "--json", "--since", "2026-06-25"]);
    }
}
