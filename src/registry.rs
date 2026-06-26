//! 编译期白名单：客户端只传 action id，真实程序与固定参数在此写死。
//! 受控参数模式：仅对显式开放的 action 接受经正则约束的值，作为独立 argv 元素传入。
//! 支持多受控参数（最多 2 个，如 cc_daily_range 的 since + until），按数组顺序追加。

use std::process::Command;

/// 白名单条目。
pub struct Entry {
    pub program: &'static str,
    pub args: &'static [&'static str],
    /// 受控参数列表，最多 2 个（如 since + until）。
    /// 每项 = (json 字段名, 正则 pattern, cli flag)。
    /// 为空切片则该 action 不接受任何受控参数。
    pub controlled: &'static [(&'static str, &'static str, &'static str)],
    /// 是否将命令 stdout 当作 JSON 解析后放入 data 字段。
    pub parses_json: bool,
}

/// 解析 action id 为白名单条目。未命中返回 None（调用方返回 404）。
///
/// 注意：受控参数的校验仅做格式约束，绝不拼接 shell 字符串。
pub fn resolve(id: &str) -> Option<Entry> {
    let e = |program, args, controlled, parses_json| Entry {
        program,
        args,
        controlled,
        parses_json,
    };
    match id {
        // ============ RTK 分析类 ============
        "rtk_gain" => Some(e(
            "rtk",
            &["gain", "--all", "--format", "json"],
            &[],
            true,
        )),
        "rtk_gain_daily" => Some(e(
            "rtk",
            &["gain", "--daily", "--format", "json"],
            &[],
            true,
        )),
        "rtk_gain_history" => Some(e(
            "rtk",
            &["gain", "--history", "--format", "json"],
            &[],
            true,
        )),
        "rtk_discover" => Some(e(
            "rtk",
            &["discover", "--all", "--since", "7", "--format", "json"],
            &[],
            true,
        )),
        "rtk_discover_at" => Some(e(
            "rtk",
            &["discover", "--all", "--format", "json"],
            &[("days", r"^(?:[1-9]|[1-8][0-9]|90)$", "--since")],
            true,
        )),
        "rtk_session" => Some(e("rtk", &["session", "--format", "json"], &[], true)),
        "rtk_version" => Some(e("rtk", &["--version"], &[], false)),

        // ============ ccusage 分析类 ============
        "cc_daily" => Some(e("ccusage", &["daily", "--json"], &[], true)),
        "cc_weekly" => Some(e("ccusage", &["weekly", "--json"], &[], true)),
        "cc_monthly" => Some(e("ccusage", &["monthly", "--json"], &[], true)),
        "cc_session" => Some(e("ccusage", &["session", "--json"], &[], true)),
        "cc_blocks" => Some(e("ccusage", &["blocks", "--json"], &[], true)),
        "cc_no_cost_daily" => Some(e(
            "ccusage",
            &["daily", "--json", "--no-cost"],
            &[],
            true,
        )),
        "cc_claude_daily" => Some(e("ccusage", &["claude", "daily", "--json"], &[], true)),
        "cc_codex_daily" => Some(e("ccusage", &["codex", "daily", "--json"], &[], true)),
        "cc_gemini_daily" => Some(e("ccusage", &["gemini", "daily", "--json"], &[], true)),
        "cc_copilot_daily" => Some(e("ccusage", &["copilot", "daily", "--json"], &[], true)),
        "cc_daily_at" => Some(e(
            "ccusage",
            &["daily", "--json"],
            &[("since", r"^\d{4}-\d{2}-\d{2}$", "--since")],
            true,
        )),
        "cc_daily_range" => Some(e(
            "ccusage",
            &["daily", "--json"],
            &[
                ("since", r"^\d{4}-\d{2}-\d{2}$", "--since"),
                ("until", r"^\d{4}-\d{2}-\d{2}$", "--until"),
            ],
            true,
        )),

        _ => None,
    }
}

/// 简易正则：仅支持本白名单用到的「完全匹配 + 锚定 + 字符类」子集。
/// 为避免引入 regex crate（依赖约束），这里手写一个最小校验器。
/// 仅用于受控参数格式约束，不用于通用匹配。
pub fn matches_simple(pattern: &str, value: &str) -> bool {
    match pattern {
        // 日期 YYYY-MM-DD（两种等价写法都支持）
        r"^\d{4}-\d{2}-\d{2}$" | "^[0-9]{4}-[0-9]{2}-[0-9]{2}$" => {
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
            true
        }
        // RTK --since 天数：1-90 整数
        r"^(?:[1-9]|[1-8][0-9]|90)$" => value
            .parse::<u8>()
            .map(|n| (1..=90).contains(&n))
            .unwrap_or(false),
        _ => false,
    }
}

/// 构造已就绪的 Command（仅 new + args，绝不经过 shell）。
/// 受控参数从 `params` JSON 中按 `entry.controlled` 顺序提取，
/// 经正则校验后作为独立 argv 元素追加（flag 与值各自独立 arg）。
/// 任一受控参数缺失或格式不符，返回 None（调用方返回 400）。
pub fn build_command(entry: &Entry, params: &serde_json::Value) -> Option<Command> {
    let mut cmd = Command::new(entry.program);
    for a in entry.args {
        cmd.arg(a);
    }
    for (field, regex, flag) in entry.controlled {
        let v = params.get(field).and_then(|v| v.as_str())?;
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
    fn all_actions_resolve() {
        for id in [
            "rtk_gain",
            "rtk_gain_daily",
            "rtk_gain_history",
            "rtk_discover",
            "rtk_discover_at",
            "rtk_session",
            "rtk_version",
            "cc_daily",
            "cc_weekly",
            "cc_monthly",
            "cc_session",
            "cc_blocks",
            "cc_no_cost_daily",
            "cc_claude_daily",
            "cc_codex_daily",
            "cc_gemini_daily",
            "cc_copilot_daily",
            "cc_daily_at",
            "cc_daily_range",
        ] {
            assert!(resolve(id).is_some(), "missing: {id}");
        }
        assert!(resolve("rm_rf").is_none());
        assert!(resolve("").is_none());
        // 已删除的 rtk_gain_graph 不应再可解析
        assert!(resolve("rtk_gain_graph").is_none(), "rtk_gain_graph 应已删除");
    }

    #[test]
    fn date_format_validation() {
        assert!(matches_simple("^[0-9]{4}-[0-9]{2}-[0-9]{2}$", "2026-06-25"));
        assert!(!matches_simple("^[0-9]{4}-[0-9]{2}-[0-9]{2}$", "2026-6-25"));
        assert!(!matches_simple("^[0-9]{4}-[0-9]{2}-[0-9]{2}$", "abcd-ef-gh"));
        assert!(!matches_simple(
            "^[0-9]{4}-[0-9]{2}-[0-9]{2}$",
            "2026-06-25; rm -rf /"
        ));
        // \d 写法等价
        assert!(matches_simple(r"^\d{4}-\d{2}-\d{2}$", "2026-06-25"));
    }

    #[test]
    fn days_format_validation() {
        assert!(matches_simple(r"^(?:[1-9]|[1-8][0-9]|90)$", "7"));
        assert!(matches_simple(r"^(?:[1-9]|[1-8][0-9]|90)$", "90"));
        assert!(!matches_simple(r"^(?:[1-9]|[1-8][0-9]|90)$", "0"));
        assert!(!matches_simple(r"^(?:[1-8][0-9]|90)$", "91"));
        assert!(!matches_simple(r"^(?:[1-9]|[1-8][0-9]|90)$", "7; rm -rf /"));
        assert!(!matches_simple(r"^(?:[1-9]|[1-8][0-9]|90)$", "abc"));
    }

    #[test]
    fn cc_daily_at_builds_single_since() {
        let entry = resolve("cc_daily_at").unwrap();
        let p = serde_json::json!({"since": "2026-06-25"});
        let cmd = build_command(&entry, &p).unwrap();
        let args: Vec<_> = cmd.get_args().map(|s| s.to_string_lossy().to_string()).collect();
        assert_eq!(args, vec!["daily", "--json", "--since", "2026-06-25"]);
    }

    #[test]
    fn cc_daily_at_rejects_bad_format() {
        let entry = resolve("cc_daily_at").unwrap();
        let p = serde_json::json!({"since": "not-a-date"});
        assert!(build_command(&entry, &p).is_none());
        // 缺字段
        let p = serde_json::json!({});
        assert!(build_command(&entry, &p).is_none());
    }

    #[test]
    fn cc_daily_range_two_params() {
        let entry = resolve("cc_daily_range").unwrap();
        let p = serde_json::json!({"since": "2026-01-01", "until": "2026-06-25"});
        let cmd = build_command(&entry, &p).unwrap();
        let args: Vec<_> = cmd.get_args().map(|s| s.to_string_lossy().to_string()).collect();
        assert_eq!(
            args,
            vec!["daily", "--json", "--since", "2026-01-01", "--until", "2026-06-25"]
        );
    }

    #[test]
    fn cc_daily_range_rejects_partial_or_bad() {
        let entry = resolve("cc_daily_range").unwrap();
        // 缺 until
        let p = serde_json::json!({"since": "2026-01-01"});
        assert!(build_command(&entry, &p).is_none());
        // until 格式错
        let p = serde_json::json!({"since": "2026-01-01", "until": "bad"});
        assert!(build_command(&entry, &p).is_none());
        // 注入尝试
        let p = serde_json::json!({"since": "2026-01-01", "until": "2026-06-25; rm -rf /"});
        assert!(build_command(&entry, &p).is_none());
    }

    #[test]
    fn rtk_discover_at_days_validation() {
        let entry = resolve("rtk_discover_at").unwrap();
        assert!(build_command(&entry, &serde_json::json!({"days": "7"})).is_some());
        assert!(build_command(&entry, &serde_json::json!({"days": "90"})).is_some());
        assert!(build_command(&entry, &serde_json::json!({"days": "0"})).is_none());
        assert!(build_command(&entry, &serde_json::json!({"days": "91"})).is_none());
        assert!(build_command(&entry, &serde_json::json!({"days": "7; rm -rf /"})).is_none());
    }

    #[test]
    fn rtk_discover_at_builds_correct_order() {
        let entry = resolve("rtk_discover_at").unwrap();
        let p = serde_json::json!({"days": "30"});
        let cmd = build_command(&entry, &p).unwrap();
        let args: Vec<_> = cmd.get_args().map(|s| s.to_string_lossy().to_string()).collect();
        assert_eq!(
            args,
            vec!["discover", "--all", "--format", "json", "--since", "30"]
        );
    }

    #[test]
    fn non_controlled_action_ignores_params() {
        let entry = resolve("rtk_gain").unwrap();
        let p = serde_json::json!({"anything": "ignored"});
        let cmd = build_command(&entry, &p).unwrap();
        let args: Vec<_> = cmd.get_args().map(|s| s.to_string_lossy().to_string()).collect();
        assert_eq!(args, vec!["gain", "--all", "--format", "json"]);
    }
}
