//! Response-header allowlist. Never retain arbitrary upstream headers.
use chrono::{DateTime, Duration, Utc};
use lazoo_protocol::{AccountApiProtocol, UsageWindow};
use reqwest::header::HeaderMap;
use uuid::Uuid;

pub(super) fn windows(
    headers: &HeaderMap,
    protocol: AccountApiProtocol,
    provider: &str,
    account_id: Uuid,
) -> Vec<UsageWindow> {
    let now = Utc::now();
    [
        "requests",
        "tokens",
        "input-tokens",
        "output-tokens",
        "project-tokens",
    ]
    .into_iter()
    .filter_map(|kind| {
        let name = |field| match protocol {
            AccountApiProtocol::Responses => format!("x-ratelimit-{field}-{kind}"),
            AccountApiProtocol::Anthropic => format!("anthropic-ratelimit-{kind}-{field}"),
        };
        let get = |field| {
            headers
                .get(name(field))
                .and_then(|value| value.to_str().ok())
        };
        let limit = get("limit").and_then(|value| value.parse::<u64>().ok());
        let remaining = get("remaining").and_then(|value| value.parse::<u64>().ok());
        let resets_at = get("reset")
            .and_then(|value| match protocol {
                AccountApiProtocol::Anthropic => DateTime::parse_from_rfc3339(value)
                    .ok()
                    .map(|date| date.with_timezone(&Utc)),
                AccountApiProtocol::Responses => duration_millis(value)
                    .and_then(|value| now.checked_add_signed(Duration::milliseconds(value))),
            })
            .map(|date| date.to_rfc3339());
        if limit.is_none() && remaining.is_none() && resets_at.is_none() {
            return None;
        }
        Some(UsageWindow {
            provider: provider.into(),
            account_id: Some(account_id),
            kind: kind.into(),
            used_percent: used_percent(limit, remaining),
            remaining,
            limit,
            window_minutes: None,
            resets_at,
            observed_at: now.to_rfc3339(),
            source: match protocol {
                AccountApiProtocol::Responses => "openai-headers",
                AccountApiProtocol::Anthropic => "anthropic-headers",
            }
            .into(),
            plan_type: None,
        })
    })
    .collect()
}

#[allow(clippy::cast_precision_loss)] // Percentages deliberately trade integer precision for a bounded ratio.
fn used_percent(limit: Option<u64>, remaining: Option<u64>) -> Option<f64> {
    let limit = limit.filter(|value| *value > 0)?;
    let remaining = remaining?.min(limit);
    Some(100.0 * (limit - remaining) as f64 / limit as f64)
}

pub(super) fn retry_after(headers: &HeaderMap, windows: &[UsageWindow]) -> u64 {
    let now = Utc::now();
    let retry = headers
        .get("retry-after")
        .and_then(|value| value.to_str().ok())
        .and_then(|value| {
            value.parse::<u64>().ok().or_else(|| {
                DateTime::parse_from_rfc2822(value).ok().and_then(|date| {
                    u64::try_from((date.with_timezone(&Utc) - now).num_seconds().max(1)).ok()
                })
            })
        });
    retry
        .or_else(|| {
            windows
                .iter()
                .filter(|window| window.used_percent == Some(100.0))
                .filter_map(|window| {
                    window
                        .resets_at
                        .as_deref()
                        .and_then(|value| DateTime::parse_from_rfc3339(value).ok())
                })
                .filter_map(|date| {
                    u64::try_from((date.with_timezone(&Utc) - now).num_seconds().max(1)).ok()
                })
                .max()
        })
        .unwrap_or(60)
        .clamp(1, 86_400)
}

fn duration_millis(value: &str) -> Option<i64> {
    if value.is_empty() || value.len() > 64 {
        return None;
    }
    let mut rest = value;
    let mut total = 0_i64;
    while !rest.is_empty() {
        let end = rest.find(|c: char| !c.is_ascii_digit() && c != '.')?;
        let number = rest[..end].parse::<f64>().ok()?;
        let units = &rest[end..];
        let (unit, multiplier) = [
            ("ms", 1.0),
            ("s", 1000.0),
            ("m", 60_000.0),
            ("h", 3_600_000.0),
            ("d", 86_400_000.0),
        ]
        .into_iter()
        .find(|(unit, _)| units.starts_with(unit))?;
        let millis = number * multiplier;
        if !millis.is_finite() || !(0.0..=2_592_000_000.0).contains(&millis) {
            return None;
        }
        #[allow(clippy::cast_possible_truncation)] // Finite bounded duration, rounded upward.
        let millis = millis.ceil() as i64;
        total = total.checked_add(millis)?;
        if total > 2_592_000_000 {
            return None;
        }
        rest = &units[unit.len()..];
    }
    Some(total)
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn parses_documented_headers_and_excludes_secrets() {
        let mut headers = HeaderMap::new();
        for (key, value) in [
            ("authorization", "secret"),
            ("x-ratelimit-limit-requests", "100"),
            ("x-ratelimit-remaining-requests", "20"),
            ("x-ratelimit-reset-requests", "6m0.5s"),
        ] {
            headers.insert(key, value.parse().unwrap());
        }
        let observed = windows(
            &headers,
            AccountApiProtocol::Responses,
            "codex",
            Uuid::new_v4(),
        );
        assert_eq!(observed.len(), 1);
        assert_eq!(observed[0].used_percent, Some(80.0));
        assert!(observed[0].resets_at.is_some());
        assert!(!serde_json::to_string(&observed).unwrap().contains("secret"));
        assert_eq!(duration_millis("1h2m3s4ms"), Some(3_723_004));
        assert_eq!(duration_millis("NaNms"), None);
        assert_eq!(duration_millis("999999999999d"), None);
        headers.insert("retry-after", "120".parse().unwrap());
        assert_eq!(retry_after(&headers, &observed), 120);
    }
}
