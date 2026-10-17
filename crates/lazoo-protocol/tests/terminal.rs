use lazoo_protocol::{SessionTerminalAttachParams, SessionTerminalAttachResult, TerminalRecord};
use serde_json::json;
use uuid::Uuid;

#[test]
fn session_terminal_attach_contracts_are_strict_camel_case_and_wrap_a_plain_terminal_record() {
    let session_id = Uuid::new_v4();
    let params: SessionTerminalAttachParams = serde_json::from_value(json!({
        "id": session_id,
        "expectedRevision": 4,
        "columns": 120,
        "rows": 40,
        "idempotencyKey": "attach-rhea"
    }))
    .unwrap();
    assert_eq!(params.id, session_id);
    assert_eq!(params.expected_revision, 4);
    assert_eq!((params.columns, params.rows), (120, 40));
    assert!(
        serde_json::from_value::<SessionTerminalAttachParams>(json!({
            "id": session_id,
            "expectedRevision": 4,
            "columns": 120,
            "rows": 40,
            "idempotencyKey": "attach-rhea",
            "authority": "full-autonomy"
        }))
        .is_err(),
        "the attach request carries no authority override"
    );

    let terminal = TerminalRecord {
        id: Uuid::new_v4(),
        boot_id: Uuid::new_v4(),
        pid: 4242,
        executable: r"C:\tools\codex.exe".to_owned(),
        args: vec!["resume".to_owned(), "thread-1".to_owned()],
        cwd: r"C:\lazoo\sessions\one".to_owned(),
        columns: 120,
        rows: 40,
        revision: 1,
        live: true,
        exit_code: None,
    };
    let result = SessionTerminalAttachResult {
        session_id,
        terminal: terminal.clone(),
    };
    let value = serde_json::to_value(&result).unwrap();
    assert_eq!(value["sessionId"], json!(session_id));
    assert_eq!(value["terminal"], serde_json::to_value(&terminal).unwrap());
    assert_eq!(
        serde_json::from_value::<SessionTerminalAttachResult>(value).unwrap(),
        result
    );
}
