//! Narrow, public quota normalization; never copy provider account metadata or error text.
use chrono::{DateTime, Utc};
use lazoo_protocol::UsageWindow;
use serde_json::Value;

use crate::{ProviderEvent, ProviderKind};

fn epoch(value: &Value) -> Option<String> {
    DateTime::from_timestamp(value.as_i64()?, 0).map(|date| date.to_rfc3339())
}

fn percent(value: &Value) -> Option<f64> {
    value
        .as_f64()
        .filter(|value| value.is_finite() && *value >= 0.0)
}

pub(crate) fn codex(snapshot: &Value, session_id: String) -> ProviderEvent {
    let snapshot = snapshot.get("rateLimits").unwrap_or(snapshot);
    let plan_type = snapshot
        .get("planType")
        .and_then(Value::as_str)
        .filter(|value| {
            matches!(
                *value,
                "free"
                    | "plus"
                    | "pro"
                    | "team"
                    | "business"
                    | "enterprise"
                    | "edu"
                    | "unknown"
                    | "go"
            )
        });
    // Other limit IDs can describe model-specific pools; don't treat those as this
    // account's general Codex quota or overwrite the primary subscription window.
    let supported = snapshot
        .get("limitId")
        .and_then(Value::as_str)
        .is_none_or(|id| id == "codex");
    let windows = ["primary", "secondary"]
        .into_iter()
        .filter_map(|kind| {
            if !supported {
                return None;
            }
            let window = snapshot.get(kind)?.as_object()?;
            Some(UsageWindow {
                provider: "codex".into(),
                account_id: None,
                kind: kind.into(),
                used_percent: window.get("usedPercent").and_then(percent),
                remaining: None,
                limit: None,
                window_minutes: window
                    .get("windowDurationMins")
                    .and_then(Value::as_u64)
                    .and_then(|value| u32::try_from(value).ok())
                    .filter(|value| *value <= 43_200),
                resets_at: window.get("resetsAt").and_then(epoch),
                observed_at: Utc::now().to_rfc3339(),
                source: "codex-app-server".into(),
                plan_type: plan_type.map(str::to_owned),
            })
        })
        .collect();
    ProviderEvent::Quota {
        provider: ProviderKind::Codex,
        session_id,
        windows,
        exhausted: false,
        raw: None,
    }
}

pub(crate) fn claude(message: &Value, session_id: String) -> ProviderEvent {
    let info = &message["rate_limit_info"];
    let exhausted = info["status"] == "rejected"
        && !matches!(
            info["overageStatus"].as_str(),
            Some("allowed" | "allowed_warning")
        );
    let (kind, minutes) = match info["rateLimitType"].as_str() {
        Some("five_hour") => ("five-hour", Some(300)),
        Some("seven_day") => ("weekly", Some(10_080)),
        Some("seven_day_opus") => ("weekly-opus", Some(10_080)),
        Some("seven_day_sonnet") => ("weekly-sonnet", Some(10_080)),
        _ => ("subscription", None),
    };
    let used_percent = if exhausted {
        Some(100.0)
    } else {
        info["utilization"]
            .as_f64()
            .map(|value| value * 100.0)
            .filter(|value| value.is_finite() && *value >= 0.0)
    };
    let windows = vec![UsageWindow {
        provider: "claude-code".into(),
        account_id: None,
        kind: kind.into(),
        used_percent,
        remaining: None,
        limit: None,
        window_minutes: minutes,
        resets_at: epoch(&info["resetsAt"]),
        observed_at: Utc::now().to_rfc3339(),
        source: "claude-stream".into(),
        plan_type: None,
    }];
    ProviderEvent::Quota {
        provider: ProviderKind::ClaudeCode,
        session_id,
        windows,
        exhausted,
        raw: None,
    }
}

pub(crate) fn codex_exhaustion(message: &Value, session_id: &str) -> Option<ProviderEvent> {
    let error = message
        .pointer("/params/turn/error/codexErrorInfo")
        .and_then(Value::as_str)?;
    matches!(error, "usageLimitExceeded" | "rateLimitExceeded").then(|| ProviderEvent::Quota {
        provider: ProviderKind::Codex,
        session_id: session_id.to_owned(),
        windows: Vec::new(),
        exhausted: true,
        raw: None,
    })
}

pub(crate) fn claude_statusline(message: &Value) -> Option<ProviderEvent> {
    let session_id = message.get("session_id")?.as_str()?.to_owned();
    if session_id.is_empty() || session_id.len() > 1024 {
        return None;
    }
    let windows: Vec<_> = [
        ("five_hour", "five-hour", 300),
        ("seven_day", "weekly", 10_080),
    ]
    .into_iter()
    .filter_map(|(field, kind, minutes)| {
        let window = message.get("rate_limits")?.get(field)?;
        Some(UsageWindow {
            provider: "claude-code".into(),
            account_id: None,
            kind: kind.into(),
            used_percent: percent(&window["used_percentage"]),
            remaining: None,
            limit: None,
            window_minutes: Some(minutes),
            resets_at: epoch(&window["resets_at"]),
            observed_at: Utc::now().to_rfc3339(),
            source: "claude-statusline".into(),
            plan_type: None,
        })
    })
    .collect();
    (!windows.is_empty()).then_some(ProviderEvent::Quota {
        provider: ProviderKind::ClaudeCode,
        session_id,
        windows,
        exhausted: false,
        raw: None,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn documented_codex_snapshot_and_notification_are_redacted() {
        let snapshot = json!({"accountId":"private-account-id","rateLimits":{"limitId":"codex","planType":"pro","credits":{"balance":"secret"},"primary":{"usedPercent":80.5,"windowDurationMins":300,"resetsAt":2_000_000_000},"secondary":{"usedPercent":20,"windowDurationMins":10_080,"resetsAt":2_000_600_000}}});
        let event = codex(&snapshot, "thread".into());
        let ProviderEvent::Quota { windows, .. } = &event else {
            panic!("quota expected")
        };
        assert_eq!(windows.len(), 2);
        assert_eq!(windows[0].used_percent, Some(80.5));
        assert_eq!(windows[0].window_minutes, Some(300));
        assert_eq!(percent(&json!(105)), Some(105.0));
        assert_eq!(percent(&json!(-1)), None);
        let serialized = serde_json::to_string(&event).unwrap();
        assert!(!serialized.contains("secret"));
        assert!(!serialized.contains("private-account-id"));
        let notification = crate::normalize_codex_message(
            &json!({"method":"account/rateLimits/updated","params":snapshot}),
        )
        .unwrap()
        .unwrap();
        assert!(matches!(notification, ProviderEvent::Quota { .. }));
        assert!(codex_exhaustion(&json!({"params":{"turn":{"error":{"codexErrorInfo":"usageLimitExceeded","message":"private-token"}}}}),"thread").is_some());
        assert!(
            codex_exhaustion(
                &json!({"params":{"turn":{"error":{"message":"usageLimitExceeded"}}}}),
                "thread"
            )
            .is_none()
        );
    }

    #[test]
    fn claude_supported_windows_exclude_context_and_free_text() {
        let event=claude_statusline(&json!({"session_id":"native","rate_limits":{"five_hour":{"used_percentage":90,"resets_at":2_000_000_000},"seven_day":{"used_percentage":50,"resets_at":2_000_600_000}},"context_window":{"used_percentage":99},"apiKey":"secret","transcript_path":"private-path"})).unwrap();
        let ProviderEvent::Quota { windows, .. } = event else {
            panic!("quota expected")
        };
        assert_eq!(windows.len(), 2);
        assert_eq!(windows[0].kind, "five-hour");
        let serialized = serde_json::to_string(&windows).unwrap();
        assert!(!serialized.contains("secret"));
        assert!(!serialized.contains("private-path"));
        assert!(
            claude_statusline(
                &json!({"session_id":"native","context_window":{"used_percentage":100}})
            )
            .is_none()
        );
        let event = claude(
            &json!({"rate_limit_info":{"status":"rejected","rateLimitType":"five_hour","resetsAt":2_000_000_000}}),
            "native".into(),
        );
        assert!(matches!(
            event,
            ProviderEvent::Quota {
                exhausted: true,
                ..
            }
        ));
        let event = claude(
            &json!({"rate_limit_info":{"status":"rejected","overageStatus":"allowed"}}),
            "native".into(),
        );
        assert!(matches!(
            event,
            ProviderEvent::Quota {
                exhausted: false,
                ..
            }
        ));
    }
}
