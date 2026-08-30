use std::collections::{BTreeMap, HashMap};

use chrono::{DateTime, TimeZone, Utc};
use whale_protocol::{CreditSnapshot, QuotaSnapshot, QuotaWindow};

pub type HeaderMap = HashMap<String, Vec<String>>;

pub fn parse_codex_quota(headers: &HeaderMap, observed_at: DateTime<Utc>) -> QuotaSnapshot {
    let canonical = canonical_headers(headers);
    let mut grouped: BTreeMap<String, WindowBuilder> = BTreeMap::new();

    for (name, value) in &canonical {
        let lower = name.to_ascii_lowercase();
        if !lower.starts_with("x-codex-") {
            continue;
        }
        for suffix in WINDOW_SUFFIXES {
            if let Some(prefix) = lower.strip_suffix(suffix) {
                let original_prefix_len = name.len().saturating_sub(suffix.len());
                let original_prefix = name[..original_prefix_len].trim_end_matches('-');
                let key = prefix.trim_end_matches('-').to_string();
                let builder = grouped.entry(key).or_default();
                builder.name = display_window_name(original_prefix);
                apply_window_value(builder, suffix, value);
                break;
            }
        }
    }

    let mut windows = grouped
        .into_values()
        .filter_map(WindowBuilder::finish)
        .collect::<Vec<_>>();
    windows.sort_by(|left, right| left.name.cmp(&right.name));

    let credits = CreditSnapshot {
        has_credits: parse_bool(canonical.get("x-codex-credits-has-credits")),
        unlimited: parse_bool(canonical.get("x-codex-credits-unlimited")),
        balance: canonical.get("x-codex-credits-balance").cloned(),
    };
    let available = !windows.is_empty()
        || credits.has_credits.is_some()
        || credits.unlimited.is_some()
        || credits.balance.is_some();

    QuotaSnapshot {
        plan_type: canonical.get("x-codex-plan-type").cloned(),
        active_limit: canonical.get("x-codex-active-limit").cloned(),
        observed_at: available.then_some(observed_at),
        windows,
        credits,
        source: if available {
            "cpa_response_headers".into()
        } else {
            "unavailable".into()
        },
        available,
    }
}

const WINDOW_SUFFIXES: [&str; 8] = [
    "-used-percent",
    "-window-minutes",
    "-reset-after-seconds",
    "-reset-at",
    "-allowed",
    "-limit-reached",
    "-limit-name",
    "-over-secondary-limit-percent",
];

#[derive(Default)]
struct WindowBuilder {
    name: String,
    limit_name: Option<String>,
    used_percent: Option<f64>,
    window_minutes: Option<i64>,
    reset_after_seconds: Option<i64>,
    reset_at: Option<DateTime<Utc>>,
    allowed: Option<bool>,
    limit_reached: Option<bool>,
}

impl WindowBuilder {
    fn finish(self) -> Option<QuotaWindow> {
        let has_value = self.limit_name.is_some()
            || self.used_percent.is_some()
            || self.window_minutes.is_some()
            || self.reset_after_seconds.is_some()
            || self.reset_at.is_some()
            || self.allowed.is_some()
            || self.limit_reached.is_some();
        if !has_value {
            return None;
        }
        Some(QuotaWindow {
            name: self.name,
            limit_name: self.limit_name,
            used_percent: self.used_percent,
            remaining_percent: self
                .used_percent
                .map(|value| (100.0 - value).clamp(0.0, 100.0)),
            window_minutes: self.window_minutes,
            reset_after_seconds: self.reset_after_seconds,
            reset_at: self.reset_at,
            allowed: self.allowed,
            limit_reached: self.limit_reached,
        })
    }
}

fn apply_window_value(builder: &mut WindowBuilder, suffix: &str, value: &str) {
    match suffix {
        "-used-percent" => builder.used_percent = value.parse::<f64>().ok(),
        "-window-minutes" => builder.window_minutes = value.parse::<i64>().ok(),
        "-reset-after-seconds" => builder.reset_after_seconds = value.parse::<i64>().ok(),
        "-reset-at" => {
            builder.reset_at = value
                .parse::<i64>()
                .ok()
                .and_then(|seconds| Utc.timestamp_opt(seconds, 0).single())
        }
        "-allowed" => builder.allowed = parse_bool_value(value),
        "-limit-reached" => builder.limit_reached = parse_bool_value(value),
        "-limit-name" => builder.limit_name = Some(value.to_string()),
        _ => {}
    }
}

fn canonical_headers(headers: &HeaderMap) -> HashMap<String, String> {
    headers
        .iter()
        .filter_map(|(name, values)| {
            values
                .last()
                .map(|value| (name.to_ascii_lowercase(), value.trim().to_string()))
        })
        .collect()
}

fn parse_bool(value: Option<&String>) -> Option<bool> {
    value.and_then(|value| parse_bool_value(value))
}

fn parse_bool_value(value: &str) -> Option<bool> {
    match value.trim().to_ascii_lowercase().as_str() {
        "true" | "1" | "yes" => Some(true),
        "false" | "0" | "no" => Some(false),
        _ => None,
    }
}

fn display_window_name(prefix: &str) -> String {
    prefix
        .strip_prefix("X-Codex-")
        .or_else(|| prefix.strip_prefix("x-codex-"))
        .unwrap_or(prefix)
        .replace('-', " ")
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::TimeZone;

    #[test]
    fn parses_primary_additional_and_credits() {
        let headers = HeaderMap::from([
            ("X-Codex-Plan-Type".into(), vec!["pro".into()]),
            ("X-Codex-Active-Limit".into(), vec!["premium".into()]),
            ("X-Codex-Primary-Used-Percent".into(), vec!["51".into()]),
            (
                "X-Codex-Primary-Window-Minutes".into(),
                vec!["10080".into()],
            ),
            ("X-Codex-Primary-Reset-At".into(), vec!["1787588999".into()]),
            (
                "X-Codex-Additional-GPT-5-6-Sol-Limit-Name".into(),
                vec!["GPT-5.6-Sol".into()],
            ),
            (
                "X-Codex-Additional-GPT-5-6-Sol-Primary-Used-Percent".into(),
                vec!["35".into()],
            ),
            ("X-Codex-Credits-Balance".into(), vec!["5".into()]),
        ]);
        let at = Utc.with_ymd_and_hms(2026, 8, 30, 0, 0, 0).unwrap();
        let quota = parse_codex_quota(&headers, at);
        assert!(quota.available);
        assert_eq!(quota.plan_type.as_deref(), Some("pro"));
        assert_eq!(quota.credits.balance.as_deref(), Some("5"));
        assert!(quota
            .windows
            .iter()
            .any(|window| window.used_percent == Some(51.0)));
        assert!(quota
            .windows
            .iter()
            .any(|window| window.limit_name.as_deref() == Some("GPT-5.6-Sol")));
    }

    #[test]
    fn ignores_non_codex_headers() {
        let headers =
            HeaderMap::from([("X-Ratelimit-Remaining-Tokens".into(), vec!["100".into()])]);
        let quota = parse_codex_quota(&headers, Utc::now());
        assert!(!quota.available);
    }
}
