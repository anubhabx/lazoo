use lazoo_protocol::{
    EmployeeAvailability, EmployeeSessionRecord, ProviderObservation, SessionLifecycleState,
};

#[must_use]
pub fn derive_availability(
    enabled: bool,
    observation: Option<&ProviderObservation>,
) -> (EmployeeAvailability, Option<String>) {
    if !enabled {
        return (
            EmployeeAvailability::Disabled,
            Some("employee is disabled".to_owned()),
        );
    }
    let Some(observation) = observation else {
        return (
            EmployeeAvailability::Offline,
            Some("provider has not been probed".to_owned()),
        );
    };
    if observation.verification == "not-detected" {
        return (
            EmployeeAvailability::Offline,
            Some(
                observation
                    .degradation_reason
                    .clone()
                    .unwrap_or_else(|| "provider executable was not detected".to_owned()),
            ),
        );
    }
    if observation.verification == "degraded" {
        return (
            EmployeeAvailability::Offline,
            Some(
                observation
                    .degradation_reason
                    .clone()
                    .unwrap_or_else(|| "provider readiness is degraded".to_owned()),
            ),
        );
    }
    if observation
        .capabilities
        .get("session")
        .and_then(serde_json::Value::as_bool)
        != Some(true)
    {
        return (
            EmployeeAvailability::Offline,
            Some("provider does not expose session capability".to_owned()),
        );
    }

    let native = authentication_state(observation, "native");
    let external = authentication_state(observation, "external");
    let ready = native == Some("ready") || external == Some("ready");
    let blocked = ["blocked", "needs-login"]
        .iter()
        .any(|state| native == Some(state) || external == Some(state));
    if blocked && !ready {
        return (
            EmployeeAvailability::Offline,
            Some("provider authentication is not ready".to_owned()),
        );
    }
    if matches!(observation.verification.as_str(), "detected" | "unverified") {
        return (
            EmployeeAvailability::Unverified,
            Some("provider model readiness has not been verified".to_owned()),
        );
    }
    if !ready {
        return (
            EmployeeAvailability::Unverified,
            Some("provider authentication readiness is unknown".to_owned()),
        );
    }
    if matches!(observation.verification.as_str(), "configured" | "verified") {
        return (EmployeeAvailability::Dormant, None);
    }
    (
        EmployeeAvailability::Unverified,
        Some("provider verification state is unknown".to_owned()),
    )
}

#[must_use]
pub fn apply_session_availability(
    availability: EmployeeAvailability,
    availability_reason: Option<String>,
    session: Option<&EmployeeSessionRecord>,
) -> (EmployeeAvailability, Option<String>) {
    if availability != EmployeeAvailability::Dormant {
        return (availability, availability_reason);
    }
    let Some(session) = session else {
        return (availability, availability_reason);
    };
    match session.state {
        SessionLifecycleState::Starting => (EmployeeAvailability::Starting, None),
        SessionLifecycleState::Idle | SessionLifecycleState::Stopping => {
            (EmployeeAvailability::Idle, None)
        }
        SessionLifecycleState::Suspended => (EmployeeAvailability::Suspended, None),
        SessionLifecycleState::Failed => (
            EmployeeAvailability::Failed,
            session
                .failure_message
                .clone()
                .or_else(|| session.failure_code.clone()),
        ),
    }
}

fn authentication_state<'a>(
    observation: &'a ProviderObservation,
    channel: &str,
) -> Option<&'a str> {
    observation
        .authentication
        .get(channel)
        .and_then(serde_json::Value::as_str)
}
