# Native Provider Adapters Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Build and verify backend-native adapters for Codex, OpenCode, Grok Build, Antigravity/agy, and Claude Code without changing the desktop UI.

**Architecture:** A new `lazoo-providers` crate owns the versioned adapter contract, truthful Windows discovery/probing, provider transports, event normalization, and a registry. The daemon and CLI expose bounded read-only provider truth plus an explicit single-turn smoke diagnostic; persistent employee ownership remains out of scope.

**Tech Stack:** Rust 1.98, Tokio 1.53, Serde/serde_json, official Agent Client Protocol Rust SDK 2.0, Windows persisted environment APIs/registry reads, existing Lazoo JSON-RPC named-pipe control plane.

---

### Task 1: Provider contract and Windows discovery

**Files:**
- Create: `crates/lazoo-providers/Cargo.toml`
- Create: `crates/lazoo-providers/src/lib.rs`
- Create: `crates/lazoo-providers/src/contract.rs`
- Create: `crates/lazoo-providers/src/discovery.rs`
- Create: `crates/lazoo-providers/src/redaction.rs`
- Test: `crates/lazoo-providers/tests/contract.rs`
- Modify: `Cargo.toml`

- [ ] **Step 1: Write failing contract tests**

```rust
#[test]
fn capabilities_default_to_false_and_states_are_truthful() {
    let capabilities = ProviderCapabilities::default();
    assert!(!capabilities.session);
    assert!(!capabilities.stream);
    assert!(!capabilities.cancel);
    assert!(!capabilities.resume);
    assert!(!capabilities.terminal);
    assert!(!capabilities.permission);
    assert_eq!(
        serde_json::to_string(&ProviderVerification::NotDetected).unwrap(),
        r#""not-detected""#,
    );
    assert_eq!(
        serde_json::to_string(&ProviderVerification::Unverified).unwrap(),
        r#""unverified""#,
    );
}

#[test]
fn discovery_deduplicates_canonical_paths_and_keeps_source_priority() {
    let candidates = rank_candidates(vec![
        ExecutableCandidate::new(r"C:\Tools\codex.exe", DiscoverySource::KnownLocation),
        ExecutableCandidate::new(r"c:\tools\CODEX.exe", DiscoverySource::InheritedPath),
    ]);
    assert_eq!(candidates.len(), 1);
    assert_eq!(candidates[0].source, DiscoverySource::InheritedPath);
}

#[test]
fn redaction_removes_nested_secret_like_values() {
    let value = json!({
        "token": "secret-token",
        "nested": {"apiKey": "secret-key", "authorization": "Bearer secret"},
        "message": "safe"
    });
    assert_eq!(
        redact_json(value),
        json!({
            "token": "[REDACTED]",
            "nested": {"apiKey": "[REDACTED]", "authorization": "[REDACTED]"},
            "message": "safe"
        }),
    );
}
```

- [ ] **Step 2: Run the focused tests and verify RED**

Run: `cargo test -p lazoo-providers --test contract`

Expected: FAIL because the new crate/types/functions do not yet exist.

- [ ] **Step 3: Implement the minimal versioned contract**

Define `ADAPTER_CONTRACT_V1`, `ProviderKind`, `ProviderCapabilities`, `ProviderVerification`, `AuthenticationState`, `ExecutableCandidate`, `ProviderProbe`, `SessionRequest`, `TurnRequest`, `ProviderEvent`, `ProviderError`, `ProviderAdapter`, and Windows discovery/redaction helpers. Keep capabilities opt-in and errors stable/redacted.

- [ ] **Step 4: Run focused and workspace tests**

Run: `cargo test -p lazoo-providers --test contract && cargo test --workspace`

Expected: PASS with zero failures.

- [ ] **Step 5: Commit the contract slice**

```powershell
git add Cargo.toml Cargo.lock crates/lazoo-providers docs/superpowers/specs/2026-08-30-provider-adapters-design.md docs/superpowers/plans/2026-08-30-provider-adapters.md
git commit -m "feat: define native provider adapter contract"
```

### Task 2: Codex App Server adapter

**Files:**
- Create: `crates/lazoo-providers/src/json_rpc.rs`
- Create: `crates/lazoo-providers/src/codex.rs`
- Test: `crates/lazoo-providers/tests/codex.rs`
- Create: `crates/lazoo-providers/tests/fixtures/codex/*.jsonl`

- [ ] **Step 1: Write failing Codex fixture tests**

```rust
#[test]
fn codex_launches_app_server_over_stdio() {
    let command = CodexAdapter::command(Path::new(r"C:\Tools\codex.exe"));
    assert_eq!(command.executable, PathBuf::from(r"C:\Tools\codex.exe"));
    assert_eq!(command.args, ["app-server", "--listen", "stdio://"]);
}

#[test]
fn codex_notifications_normalize_turn_items_and_completion() {
    let lines = include_str!("fixtures/codex/turn.jsonl");
    let events = lines
        .lines()
        .map(|line| normalize_codex_message(&serde_json::from_str(line).unwrap()))
        .collect::<Result<Vec<_>, _>>()
        .unwrap();
    assert!(matches!(events[0], Some(ProviderEvent::TurnStarted { .. })));
    assert!(matches!(events[1], Some(ProviderEvent::TextDelta { .. })));
    assert!(matches!(events[2], Some(ProviderEvent::TurnCompleted { .. })));
}

#[test]
fn codex_interrupt_correlates_the_active_turn() {
    assert_eq!(
        CodexAdapter::interrupt_params("thread-1", "turn-1"),
        json!({"threadId": "thread-1", "turnId": "turn-1"}),
    );
}
```

- [ ] **Step 2: Verify RED**

Run: `cargo test -p lazoo-providers --test codex`

Expected: FAIL because `CodexAdapter` and its JSON-RPC transport are absent.

- [ ] **Step 3: Implement the Codex adapter**

Launch structured args `app-server --listen stdio://`; implement initialize/initialized, account probe, thread start/resume, turn start, notifications, interrupt, and bounded stop. Reject malformed response IDs and redact raw payloads.

- [ ] **Step 4: Verify Codex tests and live handshake**

Run: `cargo test -p lazoo-providers --test codex`

Run: `powershell -NoProfile -ExecutionPolicy Bypass -File scripts/acceptance/provider-adapters.ps1 -Provider codex -HandshakeOnly`

Expected: fixture tests PASS; installed Codex version and App Server handshake are recorded without a model call.

- [ ] **Step 5: Commit Codex**

```powershell
git add crates/lazoo-providers scripts/acceptance/provider-adapters.ps1
git commit -m "feat: connect Codex App Server"
```

### Task 3: OpenCode ACP adapter

**Files:**
- Create: `crates/lazoo-providers/src/acp.rs`
- Create: `crates/lazoo-providers/src/opencode.rs`
- Test: `crates/lazoo-providers/tests/opencode.rs`
- Create: `crates/lazoo-providers/tests/fixtures/opencode/*.jsonl`

- [ ] **Step 1: Write failing OpenCode tests**

```rust
#[test]
fn opencode_launches_acp_for_the_requested_workspace() {
    let request = SessionRequest::new(r"C:\repo");
    let command = OpenCodeAdapter::command(Path::new(r"C:\Tools\opencode.exe"), &request);
    assert_eq!(command.args, ["acp"]);
    assert_eq!(command.cwd, PathBuf::from(r"C:\repo"));
}

#[test]
fn opencode_updates_normalize_text_tools_permissions_and_usage() {
    let fixture = include_str!("fixtures/opencode/session-updates.jsonl");
    let events = normalize_acp_fixture(ProviderKind::OpenCode, fixture).unwrap();
    assert!(events.iter().any(|event| matches!(event, ProviderEvent::TextDelta { .. })));
    assert!(events.iter().any(|event| matches!(event, ProviderEvent::ToolUpdate { .. })));
    assert!(events.iter().any(|event| matches!(event, ProviderEvent::PermissionRequested { .. })));
    assert!(events.iter().any(|event| matches!(event, ProviderEvent::Usage { .. })));
}

#[tokio::test(start_paused = true)]
async fn opencode_acceptance_without_progress_becomes_stalled() {
    let mut deadline = ProgressDeadline::new(Duration::from_secs(30));
    deadline.accepted();
    tokio::time::advance(Duration::from_secs(31)).await;
    assert!(matches!(deadline.check(), Err(ProviderError::Stalled { .. })));
}
```

- [ ] **Step 2: Verify RED**

Run: `cargo test -p lazoo-providers --test opencode`

Expected: FAIL because ACP/OpenCode modules are absent.

- [ ] **Step 3: Implement OpenCode over the official ACP SDK**

Launch `opencode acp`; negotiate v1; map new/load/prompt/update/permission/cancel/close; require progress before the configured deadline; preserve unrecognized additive updates as redacted raw evidence.

- [ ] **Step 4: Verify OpenCode tests and handshake**

Run: `cargo test -p lazoo-providers --test opencode`

Run: `powershell -NoProfile -ExecutionPolicy Bypass -File scripts/acceptance/provider-adapters.ps1 -Provider opencode -HandshakeOnly`

Expected: tests PASS and installed OpenCode ACP handshake is recorded.

- [ ] **Step 5: Commit OpenCode**

```powershell
git add Cargo.toml Cargo.lock crates/lazoo-providers scripts/acceptance/provider-adapters.ps1
git commit -m "feat: connect OpenCode over ACP"
```

### Task 4: Grok Build ACP adapter

**Files:**
- Create: `crates/lazoo-providers/src/grok.rs`
- Test: `crates/lazoo-providers/tests/grok.rs`
- Create: `crates/lazoo-providers/tests/fixtures/grok/*.jsonl`

- [ ] **Step 1: Write failing Grok tests**

```rust
#[test]
fn grok_launch_preserves_model_reasoning_and_permission_options() {
    let request = SessionRequest::new(r"C:\repo")
        .with_model("grok-code-fast")
        .with_effort("high")
        .with_permission_mode("default");
    let command = GrokAdapter::command(Path::new(r"C:\Tools\grok.exe"), &request);
    assert_eq!(
        command.args,
        ["agent", "stdio", "--model", "grok-code-fast", "--reasoning-effort", "high"],
    );
    assert!(!command.args.iter().any(|arg| arg == "--always-approve"));
}

#[test]
fn grok_acp_updates_normalize_terminal_permission_and_completion() {
    let fixture = include_str!("fixtures/grok/session-updates.jsonl");
    let events = normalize_acp_fixture(ProviderKind::GrokBuild, fixture).unwrap();
    assert!(events.iter().any(|event| matches!(event, ProviderEvent::TerminalUpdate { .. })));
    assert!(events.iter().any(|event| matches!(event, ProviderEvent::PermissionRequested { .. })));
    assert!(events.iter().any(|event| matches!(event, ProviderEvent::TurnCompleted { .. })));
}

#[test]
fn grok_resume_uses_negotiated_session_capability() {
    let capabilities = acp_capabilities(json!({"loadSession": true})).unwrap();
    assert!(capabilities.resume);
    let unsupported = acp_capabilities(json!({"loadSession": false})).unwrap();
    assert!(!unsupported.resume);
}
```

- [ ] **Step 2: Verify RED**

Run: `cargo test -p lazoo-providers --test grok`

Expected: FAIL because `GrokAdapter` is absent.

- [ ] **Step 3: Implement Grok over the shared ACP client**

Launch `grok agent stdio`; negotiate v1; map provider-specific command options and ACP events; expose only negotiated terminal, permission, cancel, and resume capabilities.

- [ ] **Step 4: Verify Grok tests and handshake**

Run: `cargo test -p lazoo-providers --test grok`

Run: `powershell -NoProfile -ExecutionPolicy Bypass -File scripts/acceptance/provider-adapters.ps1 -Provider grok-build -HandshakeOnly`

Expected: tests PASS and installed Grok ACP handshake is recorded.

- [ ] **Step 5: Commit Grok**

```powershell
git add crates/lazoo-providers scripts/acceptance/provider-adapters.ps1
git commit -m "feat: connect Grok Build over ACP"
```

### Task 5: Antigravity agy stream-JSON adapter

**Files:**
- Create: `crates/lazoo-providers/src/ndjson.rs`
- Create: `crates/lazoo-providers/src/antigravity.rs`
- Test: `crates/lazoo-providers/tests/antigravity.rs`
- Create: `crates/lazoo-providers/tests/fixtures/antigravity/*.jsonl`

- [ ] **Step 1: Write failing agy tests**

```rust
#[test]
fn agy_discovery_reads_persisted_user_path_and_known_location() {
    let environment = DiscoveryEnvironment::empty()
        .with_persisted_user_path(r"C:\Users\me\AppData\Local\agy\bin")
        .with_existing_file(r"C:\Users\me\AppData\Local\agy\bin\agy.exe");
    let candidates = AntigravityAdapter::discover(&environment);
    assert_eq!(candidates[0].source, DiscoverySource::PersistedUserPath);
}

#[test]
fn agy_stream_normalizes_conversation_tools_usage_and_result() {
    let fixture = include_str!("fixtures/antigravity/turn.jsonl");
    let events = normalize_agy_fixture(fixture).unwrap();
    assert!(events.iter().any(|event| matches!(event, ProviderEvent::SessionStarted { session_id, .. } if session_id == "conversation-1")));
    assert!(events.iter().any(|event| matches!(event, ProviderEvent::ToolUpdate { .. })));
    assert!(events.iter().any(|event| matches!(event, ProviderEvent::Usage { .. })));
    assert!(events.iter().any(|event| matches!(event, ProviderEvent::TurnCompleted { .. })));
}

#[test]
fn agy_resume_uses_conversation_id_without_copying_credentials() {
    let request = SessionRequest::new(r"C:\repo").resume("conversation-1");
    let command = AntigravityAdapter::command(Path::new(r"C:\Tools\agy.exe"), &request);
    assert!(command.args.windows(2).any(|pair| pair == ["--conversation", "conversation-1"]));
    assert!(command.env.keys().all(|key| !key.to_ascii_lowercase().contains("token")));
}
```

- [ ] **Step 2: Verify RED**

Run: `cargo test -p lazoo-providers --test antigravity`

Expected: FAIL because the NDJSON/agy modules are absent.

- [ ] **Step 3: Implement long-lived agy stream JSON**

Launch print mode with paired stream-json input/output, explicit project/workspace/model/effort/mode options, one input record per turn, persisted conversation ID, bounded progress/turn deadlines, cancellation via owned process stop, and redacted stderr/raw evidence.

- [ ] **Step 4: Verify agy tests and non-model probe**

Run: `cargo test -p lazoo-providers --test antigravity`

Run: `powershell -NoProfile -ExecutionPolicy Bypass -File scripts/acceptance/provider-adapters.ps1 -Provider antigravity -HandshakeOnly`

Expected: tests PASS; executable/version/command contract is recorded as unverified until live model output is observed.

- [ ] **Step 5: Commit Antigravity**

```powershell
git add crates/lazoo-providers scripts/acceptance/provider-adapters.ps1
git commit -m "feat: connect Antigravity stream JSON"
```

### Task 6: Claude Code stream-JSON adapter

**Files:**
- Create: `crates/lazoo-providers/src/claude.rs`
- Test: `crates/lazoo-providers/tests/claude.rs`
- Create: `crates/lazoo-providers/tests/fixtures/claude/*.jsonl`

- [ ] **Step 1: Write failing Claude tests**

```rust
#[test]
fn claude_launch_keeps_native_and_external_auth_evidence_separate() {
    let probe = ClaudeAdapter::probe_from_auth_fixture(include_str!("fixtures/claude/auth.json")).unwrap();
    assert_eq!(probe.authentication.native, AuthenticationState::NeedsLogin);
    assert_eq!(probe.authentication.external, AuthenticationState::Ready);
    assert_eq!(probe.verification, ProviderVerification::Configured);
}

#[test]
fn claude_stream_normalizes_system_assistant_tool_usage_and_result() {
    let fixture = include_str!("fixtures/claude/turn.jsonl");
    let events = normalize_claude_fixture(fixture).unwrap();
    assert!(events.iter().any(|event| matches!(event, ProviderEvent::SessionStarted { .. })));
    assert!(events.iter().any(|event| matches!(event, ProviderEvent::TextDelta { .. })));
    assert!(events.iter().any(|event| matches!(event, ProviderEvent::ToolUpdate { .. })));
    assert!(events.iter().any(|event| matches!(event, ProviderEvent::Usage { .. })));
    assert!(events.iter().any(|event| matches!(event, ProviderEvent::TurnCompleted { .. })));
}

#[test]
fn claude_resume_reuses_session_id_and_default_permissions_are_not_bypassed() {
    let request = SessionRequest::new(r"C:\repo").resume("9f73d9f2-93a0-4a43-a8f2-e0ff30d1c920");
    let command = ClaudeAdapter::command(Path::new(r"C:\Tools\claude.exe"), &request);
    assert!(command.args.windows(2).any(|pair| pair == ["--resume", "9f73d9f2-93a0-4a43-a8f2-e0ff30d1c920"]));
    assert!(!command.args.iter().any(|arg| arg == "--dangerously-skip-permissions"));
}
```

- [ ] **Step 2: Verify RED**

Run: `cargo test -p lazoo-providers --test claude`

Expected: FAIL because `ClaudeAdapter` is absent.

- [ ] **Step 3: Implement Claude long-lived stream JSON**

Launch print mode with stream-json input/output and replay acknowledgements, explicit UUID/session resume, model/effort/permission/workspace options, event normalization, graceful EOF/cancel behavior, and distinct native/external authentication evidence.

- [ ] **Step 4: Verify Claude tests and readiness probe**

Run: `cargo test -p lazoo-providers --test claude`

Run: `powershell -NoProfile -ExecutionPolicy Bypass -File scripts/acceptance/provider-adapters.ps1 -Provider claude-code -HandshakeOnly`

Expected: tests PASS; the acceptance record reports the current live blocker rather than upgrading it to verified.

- [ ] **Step 5: Commit Claude**

```powershell
git add crates/lazoo-providers scripts/acceptance/provider-adapters.ps1
git commit -m "feat: connect Claude Code stream JSON"
```

### Task 7: Daemon and CLI provider truth

**Files:**
- Modify: `crates/lazoo-protocol/src/types.rs`
- Modify: `crates/lazoo-protocol/src/lib.rs`
- Modify: `crates/lazoo-protocol/tests/contracts.rs`
- Modify: `apps/lazood/Cargo.toml`
- Modify: `apps/lazood/src/app.rs`
- Modify: `apps/lazood/tests/daemon.rs`
- Modify: `apps/lazooctl/src/lib.rs`
- Modify: `apps/lazooctl/tests/cli.rs`
- Modify: `README.md`

- [ ] **Step 1: Write failing protocol, daemon, and CLI tests**

```rust
#[test]
fn provider_probe_contract_is_strict_camel_case() {
    let value = serde_json::to_value(ProviderProbe {
        contract_version: ADAPTER_CONTRACT_V1,
        provider: ProviderKind::Codex,
        verification: ProviderVerification::Detected,
        executable: Some(PathBuf::from(r"C:\Tools\codex.exe")),
        discovery_source: Some(DiscoverySource::InheritedPath),
        version: Some("0.150.0-alpha.12.2".to_owned()),
        protocol: Some("app-server".to_owned()),
        capabilities: ProviderCapabilities::default(),
        authentication: AuthenticationEvidence::unknown(),
        degradation_reason: None,
        evidence: Vec::new(),
        checked_at: "2026-08-30T00:00:00Z".to_owned(),
    }).unwrap();
    assert_eq!(value["provider"], "codex");
    assert!(value.get("degradationReason").is_some());
    assert!(value.get("degradation_reason").is_none());
    let mut invalid = value;
    invalid["unexpected"] = json!(true);
    assert!(serde_json::from_value::<ProviderProbe>(invalid).is_err());
}

#[tokio::test]
async fn provider_list_reports_each_provider_without_changing_desktop_snapshot() {
    let response = app.handle(request("providers", "provider.list", json!({}))).await;
    let providers = response.result.unwrap().as_array().unwrap().to_owned();
    assert_eq!(providers.len(), 5);
    let snapshot = app.handle(request("desktop", "desktop.snapshot", json!({}))).await;
    assert!(snapshot.result.unwrap().get("providers").is_none());
}

#[test]
fn provider_cli_maps_list_probe_and_explicit_smoke_commands() {
    let list = Cli::try_parse_from(["lazooctl", "provider", "list"]).unwrap();
    assert_eq!(command_request(&list.command), ("provider.list", json!({}), false));
    let probe = Cli::try_parse_from(["lazooctl", "provider", "probe", "codex"]).unwrap();
    assert_eq!(command_request(&probe.command), ("provider.probe", json!({"provider": "codex"}), false));
    let smoke = Cli::try_parse_from(["lazooctl", "provider", "smoke", "grok-build", "--prompt", "Reply OK"]).unwrap();
    assert_eq!(command_request(&smoke.command), ("provider.smoke", json!({"provider": "grok-build", "prompt": "Reply OK"}), false));
}
```

- [ ] **Step 2: Verify RED**

Run: `cargo test -p lazoo-protocol -p lazood -p lazooctl`

Expected: FAIL because provider RPC and CLI commands are absent.

- [ ] **Step 3: Implement bounded provider RPC and CLI commands**

Add `provider.list`, `provider.probe`, and explicit `provider.smoke`; keep provider fields out of `desktop.snapshot`; map provider/config/timeout failures to stable `LZ-PROVIDER-*` errors and exit code 8.

- [ ] **Step 4: Verify focused integration tests**

Run: `cargo test -p lazoo-protocol -p lazood -p lazooctl`

Expected: PASS with zero failures and unchanged desktop contract tests.

- [ ] **Step 5: Commit backend integration**

```powershell
git add crates/lazoo-protocol apps/lazood apps/lazooctl README.md Cargo.lock
git commit -m "feat: expose truthful provider diagnostics"
```

### Task 8: Acceptance, live gates, and evidence

**Files:**
- Create: `scripts/acceptance/provider-adapters.ps1`
- Create: `docs/verification/2026-08-30-provider-adapters.md`
- Create/update generated manifest: `artifacts/acceptance/provider-adapters-manifest.json`

- [ ] **Step 1: Run fixture and full repository gates**

```powershell
& .\scripts\verify-rust-toolchain.ps1
cargo test --workspace
cargo check --workspace --all-targets
cargo clippy --workspace --all-targets -- -D warnings
cargo fmt --all -- --check
cargo build --workspace --release --locked
powershell -NoProfile -ExecutionPolicy Bypass -File scripts\acceptance\first-slice.ps1
```

Expected: every command exits 0; record exact test counts and blockers.

- [ ] **Step 2: Run provider gates one at a time**

```powershell
powershell -NoProfile -ExecutionPolicy Bypass -File scripts\acceptance\provider-adapters.ps1 -Provider codex
powershell -NoProfile -ExecutionPolicy Bypass -File scripts\acceptance\provider-adapters.ps1 -Provider opencode
powershell -NoProfile -ExecutionPolicy Bypass -File scripts\acceptance\provider-adapters.ps1 -Provider grok-build
powershell -NoProfile -ExecutionPolicy Bypass -File scripts\acceptance\provider-adapters.ps1 -Provider antigravity
powershell -NoProfile -ExecutionPolicy Bypass -File scripts\acceptance\provider-adapters.ps1 -Provider claude-code
```

Expected: each manifest entry independently reports pass, fail, blocked, or unverified. No blocked provider is called supported.

- [ ] **Step 3: Write verification evidence and self-review requirement coverage**

Record commands, exit codes, test counts, executable versions, handshake/model-backed status, blocker strings, known limitations, and the absence of UI changes in `docs/verification/2026-08-30-provider-adapters.md`.

- [ ] **Step 4: Commit final evidence**

```powershell
git add scripts/acceptance/provider-adapters.ps1 docs/verification/2026-08-30-provider-adapters.md artifacts/acceptance/provider-adapters-manifest.json
git commit -m "test: verify native provider adapters"
```
