# Lazoo Control-Plane Vertical Slice Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (- [ ]) syntax for tracking.

**Goal:** Build and verify the first runnable Windows control-plane slice: lazoo-keeper supervises lazood, lazooctl communicates over a secured named pipe, project registration persists transactionally in SQLite, and restart exhaustion uses an emergency spool.

**Architecture:** A Rust workspace separates protocol/domain logic, Windows runtime facilities, SQLite persistence, and named-pipe transport. Three thin binaries compose those libraries. The first domain mutation is idempotent project registration, producing immutable audit and outbox records that survive daemon restart.

**Tech Stack:** Rust 1.92.0; Tokio 1.53.1; rusqlite 0.40.2 with bundled SQLite; windows-rs 0.62.2; Clap 4.6.6; Serde 1.0.229; serde_json 1.0.151; UUID 1.26.0; thiserror 2.0.20; tracing 0.1.44; HMAC 0.13.0 and SHA-2 0.11.0.

**Spec:** docs/superpowers/specs/2026-08-28-control-plane-vertical-slice-design.md

## Global Constraints

- Windows 11 and the current user's x86_64-pc-windows-gnu Rust 1.92.0 toolchain are the execution baseline.
- No network listener, provider adapter, Tauri code, ConPTY, worktree creation, employee/task model, policy engine, or managed-memory processing.
- Mutable state defaults to %LOCALAPPDATA%\Lazoo and tests use LAZOO_DATA_ROOT with a temporary directory.
- The control pipe uses JSON-RPC 2.0 UTF-8 frames with a four-byte little-endian length and a 1 MiB limit.
- Pipe access is restricted to the installation owner's SID and LocalSystem, then authenticated with an HMAC challenge using a DPAPI-protected installation nonce.
- All mutations are idempotent and commit domain state, audit event, and outbox record in one SQLite transaction.
- The shipped checkout-mode default is isolated; lead-main is stored only as an explicit project setting.
- Keeper production thresholds remain three unexpected exits in five minutes with 1, 2, and 5 second backoff.
- Tests precede implementation behavior. Never report an unrun gate as passed.
- rustfmt and clippy are currently unavailable on this host; keep those as explicit blocked gates unless the toolchain is repaired without changing system state.

---

### Task 1: Workspace, protocol, and domain contracts

**Files:**
- Create: Cargo.toml
- Create: rust-toolchain.toml
- Create: rustfmt.toml
- Create: crates/lazoo-protocol/Cargo.toml
- Create: crates/lazoo-protocol/src/lib.rs
- Create: crates/lazoo-protocol/src/rpc.rs
- Create: crates/lazoo-protocol/src/types.rs
- Create: crates/lazoo-domain/Cargo.toml
- Create: crates/lazoo-domain/src/lib.rs
- Create: crates/lazoo-domain/src/project.rs

**Interfaces:**
- Produces: PROTOCOL_V1, MAX_FRAME_BYTES, JsonRpcRequest, JsonRpcResponse, RpcError, InitializeParams, InitializeResult, CheckoutMode, ProjectRecord, EventSummary, RegisterProjectParams, RegisterProjectResult, SystemStatus.
- Produces: canonicalize_project(path), inspect_git_repository(path), prepare_registration(params).

- [ ] **Step 1: Write failing protocol and domain tests**

Add tests that require strict JSON-RPC deserialization, isolated checkout defaulting, lead-main round-trip, invalid non-Git rejection, and case-normalized project identity.

    #[test]
    fn checkout_mode_defaults_to_isolated() {
        assert_eq!(CheckoutMode::default(), CheckoutMode::Isolated);
    }

    #[test]
    fn unknown_rpc_fields_are_rejected() {
        let json = r#"{"jsonrpc":"2.0","id":"1","method":"system.status","params":{},"extra":true}"#;
        assert!(serde_json::from_str::<JsonRpcRequest>(json).is_err());
    }

- [ ] **Step 2: Run tests and verify RED**

Run: cargo test -p lazoo-protocol -p lazoo-domain

Expected: FAIL because workspace crates and types do not exist.

- [ ] **Step 3: Implement minimal contracts**

Use serde deny_unknown_fields on command envelopes and tagged checkout values:

    #[derive(Clone, Copy, Debug, Default, Eq, PartialEq, Serialize, Deserialize)]
    #[serde(rename_all = "kebab-case")]
    pub enum CheckoutMode {
        #[default]
        Isolated,
        LeadMain,
    }

Project inspection must invoke Git with structured arguments:

    git -C <path> rev-parse --show-toplevel
    git -C <path> rev-parse --git-common-dir

Normalize the canonical Windows path case-insensitively for identity while retaining the display path.

- [ ] **Step 4: Run tests and verify GREEN**

Run: cargo test -p lazoo-protocol -p lazoo-domain

Expected: PASS.

- [ ] **Step 5: Commit**

    git add Cargo.toml rust-toolchain.toml rustfmt.toml crates/lazoo-protocol crates/lazoo-domain
    git commit -m "feat: add Lazoo protocol and project domain"

### Task 2: Protected installation runtime

**Files:**
- Create: crates/lazoo-runtime/Cargo.toml
- Create: crates/lazoo-runtime/src/lib.rs
- Create: crates/lazoo-runtime/src/data_root.rs
- Create: crates/lazoo-runtime/src/atomic_file.rs
- Create: crates/lazoo-runtime/src/installation.rs
- Create: crates/lazoo-runtime/src/security.rs
- Create: crates/lazoo-runtime/src/single_instance.rs
- Create: crates/lazoo-runtime/src/notification.rs

**Interfaces:**
- Consumes: UUID identifiers and serde JSON.
- Produces: DataRoot::resolve(), DataRoot::from_path(), atomic_write_json(), Installation::load_or_create(), Installation::unprotect_nonce(), OwnerSecurity::current(), OwnerSecurity::apply_to_path(), SingleInstance::acquire(), NativeNotifier::notify().

- [ ] **Step 1: Write failing runtime tests**

Cover deterministic data layout, atomic replacement, concurrent installation creation, DPAPI round-trip, stable installation ID, current-user SID discovery, and duplicate single-instance rejection.

    #[test]
    fn installation_round_trips_protected_nonce() {
        let root = DataRoot::from_path(tempdir().unwrap().path());
        let created = Installation::load_or_create(&root).unwrap();
        let reopened = Installation::load_or_create(&root).unwrap();
        assert_eq!(created.id, reopened.id);
        assert_eq!(created.unprotect_nonce().unwrap(), reopened.unprotect_nonce().unwrap());
    }

- [ ] **Step 2: Run tests and verify RED**

Run: cargo test -p lazoo-runtime

Expected: FAIL because the runtime crate is absent.

- [ ] **Step 3: Implement runtime facilities**

DataRoot creates state, runtime, spool/active.open, spool/pending, spool/reconciled, and logs directories. Installation metadata stores installation_id, pipe_name, and base64 DPAPI ciphertext, never plaintext nonce.

OwnerSecurity builds protected SDDL:

    D:P(A;;GA;;;SY)(A;;GA;;;<owner-sid>)

Apply it to runtime directories and pass its SECURITY_ATTRIBUTES to named-pipe creation. SingleInstance uses a named current-user mutex derived from the installation ID. NativeNotifier exposes a test sink and a Windows notification attempt without blocking spool durability.

- [ ] **Step 4: Run tests and verify GREEN**

Run: cargo test -p lazoo-runtime

Expected: PASS on Windows.

- [ ] **Step 5: Commit**

    git add crates/lazoo-runtime
    git commit -m "feat: add protected Lazoo runtime state"

### Task 3: SQLite event, outbox, and project store

**Files:**
- Create: migrations/0001_control_plane.sql
- Create: crates/lazoo-store/Cargo.toml
- Create: crates/lazoo-store/src/lib.rs
- Create: crates/lazoo-store/src/migration.rs
- Create: crates/lazoo-store/src/projects.rs
- Create: crates/lazoo-store/src/events.rs
- Create: crates/lazoo-store/src/emergency.rs

**Interfaces:**
- Consumes: prepared project registration, ProjectRecord, EventSummary, DataRoot.
- Produces: Store::open(), Store::register_project(), Store::list_projects(), Store::list_events(), Store::publish_pending_outbox(), Store::import_pending_incidents(), Store::status().

- [ ] **Step 1: Write failing store tests**

Tests require WAL and foreign_keys, migration checksum rejection, atomic state/audit/outbox insertion, idempotent duplicate return, conflict detection, sequence continuity after reopen, outbox replay, and exact-once emergency import.

    #[test]
    fn registration_is_atomic_and_idempotent() {
        let store = test_store();
        let first = store.register_project(input("same-key")).unwrap();
        let second = store.register_project(input("same-key")).unwrap();
        assert_eq!(first.project.id, second.project.id);
        assert_eq!(store.list_events(0, 100).unwrap().len(), 1);
        assert_eq!(store.status().unwrap().outbox_pending, 1);
    }

- [ ] **Step 2: Run tests and verify RED**

Run: cargo test -p lazoo-store

Expected: FAIL because store APIs are absent.

- [ ] **Step 3: Implement migration and repository**

Open bundled SQLite, set busy_timeout to five seconds, enable foreign_keys, set journal_mode WAL, and use TransactionBehavior::Immediate for mutations. Store the SHA-256 migration checksum in schema_migrations. registration inserts processed_commands, projects, audit_events, and outbox in one transaction.

- [ ] **Step 4: Run tests and verify GREEN**

Run: cargo test -p lazoo-store

Expected: PASS.

- [ ] **Step 5: Commit**

    git add migrations crates/lazoo-store
    git commit -m "feat: add transactional Lazoo project store"

### Task 4: Framed authenticated Windows named-pipe IPC

**Files:**
- Create: crates/lazoo-ipc/Cargo.toml
- Create: crates/lazoo-ipc/src/lib.rs
- Create: crates/lazoo-ipc/src/frame.rs
- Create: crates/lazoo-ipc/src/auth.rs
- Create: crates/lazoo-ipc/src/server.rs
- Create: crates/lazoo-ipc/src/client.rs

**Interfaces:**
- Consumes: protocol envelopes, Installation nonce, OwnerSecurity SECURITY_ATTRIBUTES.
- Produces: read_frame(), write_frame(), server_challenge(), compute_proof(), verify_client_owner(), PipeServer::bind(), PipeClient::connect(), AuthenticatedConnection.

- [ ] **Step 1: Write failing framing/auth tests**

Cover little-endian framing, 1 MiB rejection before allocation, partial reads, HMAC known vector, changed-field proof failure, incompatible version, current-user pipe connection, invalid proof, and explicit DACL inspection.

    #[tokio::test]
    async fn oversized_frame_is_rejected_before_payload_read() {
        let mut bytes = (MAX_FRAME_BYTES as u32 + 1).to_le_bytes().to_vec();
        let error = read_frame(&mut bytes.as_slice()).await.unwrap_err();
        assert_eq!(error.code(), "LZ-IPC-FRAME");
    }

- [ ] **Step 2: Run tests and verify RED**

Run: cargo test -p lazoo-ipc

Expected: FAIL because IPC APIs are absent.

- [ ] **Step 3: Implement transport and handshake**

Use Tokio ServerOptions::first_pipe_instance(true) for the first instance and create the next listening instance before dispatching a connected one. Pass the explicit SECURITY_ATTRIBUTES with create_with_security_attributes_raw. Verify the impersonated client token SID, issue a 32-byte challenge, and require the exact HMAC field order from the design.

- [ ] **Step 4: Run tests and verify GREEN**

Run: cargo test -p lazoo-ipc

Expected: PASS on Windows.

- [ ] **Step 5: Commit**

    git add crates/lazoo-ipc
    git commit -m "feat: add secured Lazoo named-pipe IPC"

### Task 5: Daemon command server and event delivery

**Files:**
- Create: apps/lazood/Cargo.toml
- Create: apps/lazood/src/main.rs
- Create: apps/lazood/src/app.rs
- Create: apps/lazood/src/commands.rs
- Create: apps/lazood/src/events.rs
- Create: apps/lazood/tests/daemon.rs

**Interfaces:**
- Consumes: PipeServer, Store, domain registration, runtime installation.
- Produces: system.status, project.register, project.list, events.list, events.subscribe, and system.shutdown JSON-RPC handlers.

- [ ] **Step 1: Write failing daemon handler tests**

Instantiate App with a temporary store and fake Git inspector. Test status, register/list, duplicate idempotency, unknown method, catch-up subscription, live event, and planned shutdown.

- [ ] **Step 2: Run tests and verify RED**

Run: cargo test -p lazood

Expected: FAIL because lazood does not exist.

- [ ] **Step 3: Implement command routing**

Startup order follows the design. Subscribe to the broadcast channel before reading the audit catch-up watermark, send only events above that watermark, and mark the matching outbox row published after the internal broker accepts it.

- [ ] **Step 4: Run tests and verify GREEN**

Run: cargo test -p lazood

Expected: PASS.

- [ ] **Step 5: Commit**

    git add apps/lazood
    git commit -m "feat: add Lazoo daemon command server"

### Task 6: CLI human and JSON surfaces

**Files:**
- Create: apps/lazooctl/Cargo.toml
- Create: apps/lazooctl/src/main.rs
- Create: apps/lazooctl/src/commands.rs
- Create: apps/lazooctl/src/output.rs
- Create: apps/lazooctl/tests/cli.rs

**Interfaces:**
- Consumes: PipeClient and protocol method types.
- Produces: status, project register/list, events list/follow, and system shutdown commands; --json output; stable exit codes.

- [ ] **Step 1: Write failing CLI parsing/output tests**

Test isolated default, explicit lead-main, stable JSON keys, stable error code, and each exit-code family.

- [ ] **Step 2: Run tests and verify RED**

Run: cargo test -p lazooctl

Expected: FAIL because lazooctl does not exist.

- [ ] **Step 3: Implement CLI**

Keep parsing separate from transport. Human output is concise; --json serializes the protocol result without ANSI. Map protocol errors to documented process exit codes.

- [ ] **Step 4: Run tests and verify GREEN**

Run: cargo test -p lazooctl

Expected: PASS.

- [ ] **Step 5: Commit**

    git add apps/lazooctl
    git commit -m "feat: add Lazoo control CLI"

### Task 7: Keeper, restart budget, and emergency spool

**Files:**
- Create: apps/lazoo-keeper/Cargo.toml
- Create: apps/lazoo-keeper/src/main.rs
- Create: apps/lazoo-keeper/src/budget.rs
- Create: apps/lazoo-keeper/src/state.rs
- Create: apps/lazoo-keeper/src/spool.rs
- Create: apps/lazoo-keeper/src/supervisor.rs
- Create: apps/lazoo-keeper/tests/keeper.rs

**Interfaces:**
- Consumes: DataRoot, Installation, SingleInstance, NativeNotifier.
- Produces: RestartBudget::record_exit(), EmergencySpool::append_and_seal(), Supervisor::run().

- [ ] **Step 1: Write failing keeper tests**

Test three exits in five minutes, ten-minute reset, 1/2/5 second backoff, atomic state, partial active recovery, 16 KiB record bound, 1 MiB rotation, eight-segment refusal without overwrite, planned-stop exit, notifier failure after durable spool, and nonzero keeper exit on exhaustion.

- [ ] **Step 2: Run tests and verify RED**

Run: cargo test -p lazoo-keeper

Expected: FAIL because keeper does not exist.

- [ ] **Step 3: Implement supervisor**

Find lazood beside the keeper executable unless LAZOO_DAEMON_PATH is set for tests. Spawn with structured arguments and inherited LAZOO_DATA_ROOT. On code 0 exit cleanly. On unexpected exits apply the fixed budget; after exhaustion, persist state, spool one deduplicated LZ-DAEMON-UNAVAILABLE record, attempt notification, seal active.open, release the lock, and exit nonzero.

- [ ] **Step 4: Run tests and verify GREEN**

Run: cargo test -p lazoo-keeper

Expected: PASS.

- [ ] **Step 5: Commit**

    git add apps/lazoo-keeper
    git commit -m "feat: add Lazoo daemon keeper"

### Task 8: End-to-end Windows acceptance

**Files:**
- Create: scripts/acceptance/first-slice.ps1
- Create: tests/fixtures/crashing-daemon/Cargo.toml
- Create: tests/fixtures/crashing-daemon/src/main.rs
- Modify: Cargo.toml
- Modify: README.md

**Interfaces:**
- Consumes: all three release binaries.
- Produces: artifacts/acceptance/first-slice-manifest.json with exact gate results.

- [ ] **Step 1: Write acceptance assertions before orchestration**

The PowerShell script fails unless every expected condition is present:

    Assert-Equal $status.protocolVersion 1 'protocol version'
    Assert-Equal $projects.Count 1 'project survived restart'
    Assert-Equal $events.Count 1 'idempotent logical registration'
    Assert-True (Test-Path $pendingIncident) 'emergency spool exists'
    Assert-Equal $importCount 1 'emergency incident imported once'

- [ ] **Step 2: Run acceptance and verify RED**

Run: powershell -NoProfile -ExecutionPolicy Bypass -File scripts/acceptance/first-slice.ps1

Expected: FAIL until binaries and orchestration are wired.

- [ ] **Step 3: Implement fixture, orchestration, and README**

Build release binaries, start keeper without arbitrary sleeps, register a temporary Git repository twice with one idempotency key, terminate the daemon, verify restart and persistence, run the crashing fixture to exhaust the keeper, inspect the sealed spool, restart the real daemon, and verify one import. Record every command, exit code, duration, version, and artifact path.

- [ ] **Step 4: Run full verification**

Run:

    cargo test --workspace
    cargo check --workspace --all-targets
    cargo build --workspace --release
    powershell -NoProfile -ExecutionPolicy Bypass -File scripts/acceptance/first-slice.ps1
    git diff --check

Expected: all available gates PASS. Record rustfmt/clippy as BLOCKED with their exact missing-component output unless repaired.

- [ ] **Step 5: Commit**

    git add Cargo.toml README.md scripts tests
    git commit -m "test: verify first Lazoo control-plane slice"

### Task 9: Final contract synchronization

**Files:**
- Modify: docs/superpowers/specs/2026-08-28-control-plane-vertical-slice-design.md
- Modify: docs/PROJECT-BRIEF.md
- Create: docs/verification/2026-08-28-first-slice.md

**Interfaces:**
- Consumes: actual build and acceptance manifests.
- Produces: exact delivered/blocked status without expanding scope.

- [ ] **Step 1: Compare implementation to every acceptance criterion**

List each criterion as PASS, FAIL, or BLOCKED with the exact command or artifact.

- [ ] **Step 2: Update documentation**

Mark the design Implemented only if all required automated gates pass and manual security/notification gates are either run or explicitly blocked. Keep unavailable gates visible.

- [ ] **Step 3: Verify documentation**

Run:

    rg -n "T[B]D|T[O]DO|F[I]XME|X[X]X" docs
    git diff --check
    git status --short

Expected: no placeholders, clean whitespace, and only intended documentation changes.

- [ ] **Step 4: Commit**

    git add docs
    git commit -m "docs: record first Lazoo slice verification"
