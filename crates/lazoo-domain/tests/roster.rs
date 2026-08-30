use lazoo_domain::{apply_session_availability, derive_availability};
use lazoo_protocol::{
    EmployeeAvailability, EmployeeSessionRecord, ProviderObservation, SessionLifecycleState,
};
use serde_json::json;

fn observation(
    verification: &str,
    session: bool,
    native_auth: &str,
    external_auth: &str,
) -> ProviderObservation {
    ProviderObservation {
        provider: "codex".to_owned(),
        verification: verification.to_owned(),
        version: Some("0.150.0".to_owned()),
        protocol: Some("app-server".to_owned()),
        capabilities: json!({"session": session}),
        authentication: json!({
            "native": native_auth,
            "external": external_auth
        }),
        degradation_reason: (verification == "degraded")
            .then(|| "protocol handshake failed".to_owned()),
        checked_at: "2026-08-30T00:00:00Z".to_owned(),
    }
}

#[test]
fn availability_requires_session_capability_and_ready_authentication() {
    let configured = observation("configured", true, "ready", "unknown");
    let missing_session = observation("configured", false, "ready", "unknown");
    let blocked = observation("configured", true, "blocked", "unknown");

    assert_eq!(
        derive_availability(false, Some(&configured)).0,
        EmployeeAvailability::Disabled
    );
    assert_eq!(
        derive_availability(true, None).0,
        EmployeeAvailability::Offline
    );
    assert_eq!(
        derive_availability(true, Some(&missing_session)).0,
        EmployeeAvailability::Offline
    );
    assert_eq!(
        derive_availability(true, Some(&blocked)).0,
        EmployeeAvailability::Offline
    );
    assert_eq!(
        derive_availability(true, Some(&configured)).0,
        EmployeeAvailability::Dormant
    );
}

#[test]
fn detected_unverified_and_unknown_authentication_are_not_route_ready() {
    for probe in [
        observation("detected", true, "ready", "unknown"),
        observation("unverified", true, "ready", "unknown"),
        observation("configured", true, "unknown", "unknown"),
    ] {
        assert_eq!(
            derive_availability(true, Some(&probe)).0,
            EmployeeAvailability::Unverified
        );
    }
}

#[test]
fn degraded_reason_is_preserved_without_raw_probe_evidence() {
    let degraded = observation("degraded", true, "ready", "unknown");
    let (availability, reason) = derive_availability(true, Some(&degraded));

    assert_eq!(availability, EmployeeAvailability::Offline);
    assert_eq!(reason.as_deref(), Some("protocol handshake failed"));
}

#[test]
fn session_state_only_overlays_a_route_ready_provider() {
    let mut session = EmployeeSessionRecord {
        id: uuid::Uuid::new_v4(),
        employee_id: uuid::Uuid::new_v4(),
        employee_configuration_revision: 1,
        provider: "codex".to_owned(),
        state: SessionLifecycleState::Starting,
        revision: 1,
        owner_boot_id: Some(uuid::Uuid::new_v4()),
        native_session_id: None,
        native_host_pid: None,
        failure_code: None,
        failure_message: None,
        created_at: "2026-08-31T00:00:00Z".to_owned(),
        updated_at: "2026-08-31T00:00:00Z".to_owned(),
        started_at: None,
        stopped_at: None,
    };

    assert_eq!(
        apply_session_availability(EmployeeAvailability::Dormant, None, Some(&session)).0,
        EmployeeAvailability::Starting
    );
    assert_eq!(
        apply_session_availability(
            EmployeeAvailability::Offline,
            Some("provider unavailable".to_owned()),
            Some(&session),
        ),
        (
            EmployeeAvailability::Offline,
            Some("provider unavailable".to_owned())
        )
    );

    session.state = SessionLifecycleState::Failed;
    session.failure_code = Some("LZ-SESSION-OWNER-LOST".to_owned());
    assert_eq!(
        apply_session_availability(EmployeeAvailability::Dormant, None, Some(&session)),
        (
            EmployeeAvailability::Failed,
            Some("LZ-SESSION-OWNER-LOST".to_owned())
        )
    );
}
