use std::time::Duration;

use base64::{Engine, engine::general_purpose::STANDARD};
use lazoo_ipc::{
    IpcError, PipeClient, PipeServer, ProofFields, compute_proof, read_frame, verify_proof,
    write_frame,
};
use lazoo_protocol::MAX_FRAME_BYTES;
use lazoo_runtime::{DataRoot, Installation, OwnerSecurity};
use tempfile::tempdir;
use tokio::{
    io::{AsyncWriteExt, duplex},
    time::timeout,
};

#[tokio::test]
async fn frame_round_trips_across_partial_writes() {
    let (mut writer, mut reader) = duplex(32);
    let payload = br#"{"jsonrpc":"2.0","id":"one"}"#.to_vec();
    let expected = payload.clone();
    tokio::spawn(async move {
        let mut encoded = Vec::new();
        encoded.extend_from_slice(&u32::try_from(payload.len()).unwrap().to_le_bytes());
        encoded.extend_from_slice(&payload);
        for chunk in encoded.chunks(3) {
            writer.write_all(chunk).await.unwrap();
        }
    });

    assert_eq!(read_frame(&mut reader).await.unwrap(), expected);
}

#[tokio::test]
async fn oversized_frame_is_rejected_before_payload_read() {
    let bytes = (u32::try_from(MAX_FRAME_BYTES).unwrap() + 1).to_le_bytes();
    let mut reader = bytes.as_slice();

    let error = read_frame(&mut reader).await.unwrap_err();

    assert_eq!(error.code(), "LZ-IPC-FRAME");
}

#[tokio::test]
async fn channel_specific_frame_limit_rejects_before_body_allocation() {
    let bytes = 16385_u32.to_le_bytes();
    let mut reader = bytes.as_slice();
    let error = lazoo_ipc::read_frame_bounded(&mut reader, 16384)
        .await
        .unwrap_err();
    assert!(matches!(
        error,
        IpcError::FrameTooLarge {
            actual: 16385,
            maximum: 16384
        }
    ));
}

#[tokio::test]
async fn write_frame_rejects_oversized_payload() {
    let (mut writer, _reader) = duplex(32);
    let payload = vec![0_u8; MAX_FRAME_BYTES + 1];

    let error = write_frame(&mut writer, &payload).await.unwrap_err();

    assert_eq!(error.code(), "LZ-IPC-FRAME");
}

#[test]
fn hmac_proof_matches_known_vector_and_binds_every_field() {
    let fields = ProofFields {
        challenge: b"challenge-1",
        installation_id: "installation-1",
        client_name: "lazooctl",
        client_version: "0.1.0",
        offered_versions: &[1],
    };
    let proof = compute_proof(b"01234567890123456789012345678901", &fields).unwrap();

    assert_eq!(
        STANDARD.encode(&proof),
        "iXK4SIfAzekPMH9Cu4q6fmOpoX6Scm8w9CSmH8R2Tbw="
    );
    assert!(verify_proof(
        b"01234567890123456789012345678901",
        &fields,
        &proof
    ));

    let changed = ProofFields {
        client_name: "different",
        ..fields
    };
    assert!(!verify_proof(
        b"01234567890123456789012345678901",
        &changed,
        &proof
    ));
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn current_user_authenticates_and_exchanges_a_frame() {
    let temporary = tempdir().unwrap();
    let root = DataRoot::from_path(temporary.path());
    let installation = Installation::load_or_create(&root).unwrap();
    let owner = OwnerSecurity::current().unwrap();
    let mut server = PipeServer::bind(&installation, &owner).unwrap();

    let server_task = tokio::spawn(async move {
        let mut connection = server.accept().await.unwrap();
        let request: serde_json::Value = connection.receive_json().await.unwrap();
        assert_eq!(request["ping"], true);
        connection
            .send_json(&serde_json::json!({"pong": true}))
            .await
            .unwrap();
    });

    let mut client = PipeClient::connect(&installation, "test-client", "0.1.0")
        .await
        .unwrap();
    client
        .send_json(&serde_json::json!({"ping": true}))
        .await
        .unwrap();
    let response: serde_json::Value = client.receive_json().await.unwrap();

    assert_eq!(response["pong"], true);
    timeout(Duration::from_secs(3), server_task)
        .await
        .unwrap()
        .unwrap();
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn invalid_nonce_is_rejected_before_application_frames() {
    let temporary = tempdir().unwrap();
    let root = DataRoot::from_path(temporary.path());
    let installation = Installation::load_or_create(&root).unwrap();
    let owner = OwnerSecurity::current().unwrap();
    let mut server = PipeServer::bind(&installation, &owner).unwrap();
    let pipe_name = installation.pipe_name().to_owned();
    let installation_id = installation.id();

    let server_task = tokio::spawn(async move { server.accept().await.unwrap_err() });
    let error = PipeClient::connect_using(
        &pipe_name,
        installation_id,
        &[7_u8; 32],
        "test-client",
        "0.1.0",
        &[1],
    )
    .await
    .unwrap_err();

    assert!(matches!(error, IpcError::Authentication));
    assert_eq!(server_task.await.unwrap().code(), "LZ-IPC-AUTH");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn incompatible_protocol_fails_cleanly_on_both_ends() {
    let temporary = tempdir().unwrap();
    let root = DataRoot::from_path(temporary.path());
    let installation = Installation::load_or_create(&root).unwrap();
    let owner = OwnerSecurity::current().unwrap();
    let mut server = PipeServer::bind(&installation, &owner).unwrap();
    let pipe_name = installation.pipe_name().to_owned();
    let installation_id = installation.id();
    let nonce = installation.unprotect_nonce().unwrap();

    let server_task = tokio::spawn(async move { server.accept().await.unwrap_err() });
    let client_error = PipeClient::connect_using(
        &pipe_name,
        installation_id,
        &nonce,
        "test-client",
        "0.1.0",
        &[99],
    )
    .await
    .unwrap_err();
    let server_error = timeout(Duration::from_secs(3), server_task)
        .await
        .expect("server must not hang on incompatible initialization")
        .unwrap();

    assert!(matches!(client_error, IpcError::ProtocolIncompatible));
    assert!(matches!(server_error, IpcError::ProtocolIncompatible));
}

#[tokio::test]
async fn pipe_dacl_contains_only_owner_and_system_grants() {
    let temporary = tempdir().unwrap();
    let root = DataRoot::from_path(temporary.path());
    let installation = Installation::load_or_create(&root).unwrap();
    let owner = OwnerSecurity::current().unwrap();
    let server = PipeServer::bind(&installation, &owner).unwrap();

    let sddl = server.effective_sddl().unwrap();

    assert!(sddl.contains(owner.sid()));
    assert!(sddl.contains("SY"));
    assert!(!sddl.contains(";;;BU)"));
    assert!(!sddl.contains(";;;AU)"));
    assert!(!sddl.contains(";;;AN)"));
    assert!(!sddl.contains(";;;NU)"));
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn cancelled_accept_preserves_the_listener() {
    let temporary = tempdir().unwrap();
    let installation =
        Installation::load_or_create(&DataRoot::from_path(temporary.path())).unwrap();
    let owner = OwnerSecurity::current().unwrap();
    let mut server = PipeServer::bind(&installation, &owner).unwrap();
    assert!(
        timeout(Duration::from_millis(10), server.accept())
            .await
            .is_err()
    );
    let (server_side, client_side) = tokio::join!(
        server.accept(),
        PipeClient::connect(&installation, "after-cancel", "0.1.0")
    );
    assert!(server_side.is_ok());
    assert!(client_side.is_ok());
}
