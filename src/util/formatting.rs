use super::json::{jf64, ju64};
use serde_json::Value;
use std::time::{SystemTime, UNIX_EPOCH};

pub(crate) fn format_secs(ms: u64) -> String {
    format!("{:.1}s", ms as f64 / 1000.0)
}

/// Compact, timezone-free recency label for a unix-epoch timestamp (seconds).
pub(crate) fn relative_time(secs: u64) -> String {
    let now = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|duration| duration.as_secs())
        .unwrap_or(0);
    let delta = now.saturating_sub(secs);
    if delta < 60 {
        "just now".to_string()
    } else if delta < 3600 {
        format!("{}m ago", delta / 60)
    } else if delta < 86_400 {
        format!("{}h ago", delta / 3600)
    } else if delta < 86_400 * 7 {
        format!("{}d ago", delta / 86_400)
    } else if delta < 86_400 * 30 {
        format!("{}w ago", delta / (86_400 * 7))
    } else {
        format!("{}mo ago", delta / (86_400 * 30))
    }
}

/// Return the first `max` characters and note how many characters were omitted.
pub(crate) fn cap_head(value: &str, max: usize) -> String {
    let count = value.chars().count();
    if count <= max {
        value.to_string()
    } else {
        let head: String = value.chars().take(max).collect();
        format!("{}\n\u{2026} ({} more chars)", head, count - max)
    }
}

/// Return the last `max` characters and note how many earlier characters were omitted.
pub(crate) fn cap_tail(value: &str, max: usize) -> String {
    let count = value.chars().count();
    if count <= max {
        value.to_string()
    } else {
        let tail: String = value.chars().skip(count - max).collect();
        format!("\u{2026} ({} earlier chars)\n{}", count - max, tail)
    }
}

/// Render a getSessionStats result as aligned text for the Stats modal.
pub(crate) fn format_session_stats(result: &Value) -> String {
    let api = result.get("apiUsage");
    let role = result.get("roleTokens");
    let resolution = result.get("resolutionTokens");
    let compact = |number: u64| {
        if number >= 1000 {
            format!("{:.1}k", number as f64 / 1000.0)
        } else {
            number.to_string()
        }
    };
    format!(
        "Context\n\
         \x20 window          {}\n\
         \x20 used            {} ({}%)\n\
         \x20 remaining       {}\n\
         \x20 completion rsv  {}\n\
         \n\
         Session\n\
         \x20 messages        {}\n\
         \x20 entries         {} ({} on path, {} compacted)\n\
         \n\
         Role tokens\n\
         \x20 system          {}\n\
         \x20 user            {}\n\
         \x20 assistant       {}\n\
         \x20 tool            {}\n\
         \n\
         Resolution\n\
         \x20 full            {}\n\
         \x20 outlined        {}\n\
         \x20 summarized      {}\n\
         \x20 pinned          {}\n\
         \n\
         API usage\n\
         \x20 input           {}\n\
         \x20 output          {}\n\
         \x20 cached          {}\n\
         \x20 total           {}\n\
         \x20 requests        {}\n\
         \x20 cost            ${:.4}",
        compact(ju64(Some(result), "contextWindow")),
        compact(ju64(Some(result), "estimatedUsed")),
        ju64(Some(result), "utilizationPercent"),
        compact(ju64(Some(result), "estimatedRemaining")),
        compact(ju64(Some(result), "completionReserve")),
        ju64(Some(result), "messageCount"),
        ju64(Some(result), "entryCount"),
        ju64(Some(result), "pathEntryCount"),
        ju64(Some(result), "compactedEntryCount"),
        compact(ju64(role, "system")),
        compact(ju64(role, "user")),
        compact(ju64(role, "assistant")),
        compact(ju64(role, "tool")),
        compact(ju64(resolution, "full")),
        compact(ju64(resolution, "outlined")),
        compact(ju64(resolution, "summarized")),
        compact(ju64(resolution, "pinned")),
        compact(ju64(api, "totalInputTokens")),
        compact(ju64(api, "totalOutputTokens")),
        compact(ju64(api, "totalCachedTokens")),
        compact(ju64(api, "totalTokens")),
        ju64(api, "requestCount"),
        jf64(api, "totalCost"),
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn format_secs_whole_second() {
        assert_eq!(format_secs(1000), "1.0s");
    }

    #[test]
    fn format_secs_fractional() {
        assert_eq!(format_secs(3200), "3.2s");
    }

    #[test]
    fn format_secs_zero() {
        assert_eq!(format_secs(0), "0.0s");
    }

    #[test]
    fn format_secs_large_value() {
        assert_eq!(format_secs(65500), "65.5s");
    }

    // ── cap_head ────────────────────────────────────────────────────────────

    #[test]
    fn cap_head_short_string_unchanged() {
        assert_eq!(cap_head("hello", 100), "hello");
    }

    #[test]
    fn cap_head_exact_length_unchanged() {
        assert_eq!(cap_head("hello", 5), "hello");
    }

    #[test]
    fn cap_head_truncates_with_suffix() {
        assert_eq!(cap_head("hello world", 5), "hello\n\u{2026} (6 more chars)");
    }

    #[test]
    fn cap_head_empty_string() {
        assert_eq!(cap_head("", 10), "");
    }

    #[test]
    fn cap_head_multibyte_chars() {
        let value = "\u{1f600}\u{1f600}\u{1f600}";
        assert_eq!(cap_head(value, 10), value);
    }

    #[test]
    fn cap_head_multibyte_truncation() {
        let value = "\u{1f600}\u{1f600}\u{1f600}\u{1f600}";
        assert_eq!(cap_head(value, 2), "\u{1f600}\u{1f600}\n\u{2026} (2 more chars)");
    }

    // ── cap_tail ────────────────────────────────────────────────────────────

    #[test]
    fn cap_tail_short_string_unchanged() {
        assert_eq!(cap_tail("hello", 100), "hello");
    }

    #[test]
    fn cap_tail_exact_length_unchanged() {
        assert_eq!(cap_tail("hello", 5), "hello");
    }

    #[test]
    fn cap_tail_truncates_with_prefix() {
        assert_eq!(cap_tail("hello world", 5), "\u{2026} (6 earlier chars)\nworld");
    }

    #[test]
    fn cap_tail_empty_string() {
        assert_eq!(cap_tail("", 10), "");
    }

    #[test]
    fn cap_tail_multibyte_chars() {
        let value = "\u{1f600}\u{1f600}\u{1f600}";
        assert_eq!(cap_tail(value, 10), value);
    }

    #[test]
    fn cap_tail_multibyte_truncation() {
        let value = "\u{1f600}\u{1f600}\u{1f600}\u{1f600}";
        assert_eq!(cap_tail(value, 2), "\u{2026} (2 earlier chars)\n\u{1f600}\u{1f600}");
    }

    // ── relative_time ─────────────────────────────────────────────────────────

    fn now_secs() -> u64 {
        SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map(|duration| duration.as_secs())
            .unwrap_or(0)
    }

    #[test]
    fn relative_time_just_now() {
        assert_eq!(relative_time(now_secs()), "just now");
    }

    #[test]
    fn relative_time_minutes_ago() {
        assert_eq!(relative_time(now_secs().saturating_sub(300)), "5m ago");
    }

    #[test]
    fn relative_time_hours_ago() {
        assert_eq!(relative_time(now_secs().saturating_sub(7200)), "2h ago");
    }

    #[test]
    fn relative_time_days_ago() {
        assert_eq!(relative_time(now_secs().saturating_sub(86_400 * 3)), "3d ago");
    }

    #[test]
    fn relative_time_weeks_ago() {
        assert_eq!(relative_time(now_secs().saturating_sub(86_400 * 14)), "2w ago");
    }

    #[test]
    fn relative_time_months_ago() {
        assert_eq!(relative_time(now_secs().saturating_sub(86_400 * 60)), "2mo ago");
    }

    #[test]
    fn relative_time_future_timestamp() {
        assert_eq!(relative_time(now_secs() + 10_000), "just now");
    }

    // ── format_session_stats ─────────────────────────────────────────────────

    #[test]
    fn format_session_stats_renders_sections_and_values() {
        let result = serde_json::json!({
            "contextWindow": 200_000,
            "estimatedUsed": 10_000,
            "utilizationPercent": 5,
            "estimatedRemaining": 190_000,
            "completionReserve": 8_000,
            "messageCount": 3,
            "entryCount": 4,
            "pathEntryCount": 2,
            "compactedEntryCount": 1,
            "roleTokens": { "system": 1000, "user": 2000, "assistant": 3000, "tool": 4000 },
            "resolutionTokens": { "full": 10, "outlined": 20, "summarized": 30, "pinned": 40 },
            "apiUsage": {
                "totalInputTokens": 5000,
                "totalOutputTokens": 6000,
                "totalCachedTokens": 7000,
                "totalTokens": 18_000,
                "requestCount": 2,
                "totalCost": 0.125
            }
        });

        let formatted = format_session_stats(&result);
        assert!(formatted.contains("Context"));
        assert!(formatted.contains("window          200.0k"));
        assert!(formatted.contains("messages        3"));
        assert!(formatted.contains("cost            $0.1250"));
    }
}
