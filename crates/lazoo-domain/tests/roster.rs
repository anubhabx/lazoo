use lazoo_domain::derive_availability;
use lazoo_protocol::{EmployeeAvailability, ProviderObservation};
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
