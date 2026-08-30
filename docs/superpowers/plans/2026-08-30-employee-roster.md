# Durable Employee Roster Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Add durable employee configuration and a revisioned provider-truth roster without changing the desktop UI.

**Architecture:** Protocol types define strict employee and roster contracts; `lazoo-domain` validates user-owned configuration; migration 2 and an employee-focused store module provide atomic state/audit/outbox semantics. The daemon derives availability from bounded non-model provider probes and exposes matching CLI commands.

**Tech Stack:** Rust 1.98, Serde, Rusqlite/SQLite WAL, Tokio, Clap, existing Lazoo JSON-RPC named-pipe control plane and provider registry.

---

### Task 1: Employee and roster contracts

**Files:**
- Modify: `crates/lazoo-protocol/src/types.rs`
- Modify: `crates/lazoo-protocol/src/lib.rs`
- Modify: `crates/lazoo-protocol/tests/contracts.rs`
- Create: `crates/lazoo-domain/src/employee.rs`
- Modify: `crates/lazoo-domain/src/lib.rs`
- Test: `crates/lazoo-domain/tests/employee.rs`

- [x] **Step 1: Write failing protocol and validation tests**

```rust
#[test]
fn employee_defaults_are_ask_isolated_and_strict() {
    let value = json!({
        "name": "Rhea",
        "provider": "codex",
        "idempotencyKey": "create-rhea"
    });
    let params: CreateEmployeeParams = serde_json::from_value(value).unwrap();
    assert_eq!(params.authority_mode, AuthorityMode::Ask);
    assert_eq!(params.checkout_policy, EmployeeCheckoutPolicy::Isolated);
    assert!(params.enabled);
    assert!(serde_json::from_value::<CreateEmployeeParams>(
        json!({"name":"Rhea","provider":"codex","idempotencyKey":"x","unknown":true})
    ).is_err());
}

#[test]
fn native_settings_reject_nested_secret_like_keys() {
    let error = validate_native_settings(&json!({"nested":{"api_key":"secret"}})).unwrap_err();
    assert_eq!(error.code(), "native-settings-secret-key");
}
```

- [x] **Step 2: Run and verify RED**

Run: `cargo test -p lazoo-protocol -p lazoo-domain`

Expected: FAIL because employee and roster types/validation do not exist.

- [x] **Step 3: Implement strict contracts and validation**

Add `AuthorityMode`, `EmployeeCheckoutPolicy`, `EmployeeAvailability`, `EmployeeRecord`, `ProviderObservation`, `RosterEntry`, `RosterSnapshot`, create/update/disable/show/refresh params, and mutation results. Add bounded validation for names, text, provider-native settings, and secret-like keys.

- [x] **Step 4: Run focused tests and verify GREEN**

Run: `cargo test -p lazoo-protocol -p lazoo-domain`

Expected: PASS with zero failures.

- [x] **Step 5: Commit**

```powershell
git add crates/lazoo-protocol crates/lazoo-domain docs/superpowers/specs/2026-08-30-employee-roster-design.md docs/superpowers/plans/2026-08-30-employee-roster.md
git commit -m "feat: define employee roster contracts"
```

### Task 2: Migration 2 and atomic employee creation

**Files:**
- Create: `migrations/0002_employee_roster.sql`
- Modify: `crates/lazoo-store/src/migration.rs`
- Create: `crates/lazoo-store/src/employee.rs`
- Modify: `crates/lazoo-store/src/lib.rs`
- Modify: `crates/lazoo-store/tests/store.rs`
- Create: `crates/lazoo-store/tests/employee.rs`

- [x] **Step 1: Write failing migration and creation tests**

```rust
#[test]
fn v1_database_upgrades_to_v2_without_rewriting_v1() {
    let store = Store::open_path(database_path()).unwrap();
    assert_eq!(store.status().unwrap().schema_version, 2);
}

#[test]
fn employee_creation_is_atomic_idempotent_and_case_insensitively_unique() {
    let first = store.create_employee(&creation("same-key", "Rhea")).unwrap();
    let replay = store.create_employee(&creation("same-key", "Rhea")).unwrap();
    assert_eq!(first, replay);
    assert_eq!(store.roster_snapshot().unwrap().revision, 1);
    assert_eq!(store.list_events(0, 100).unwrap().len(), 1);
    assert!(matches!(
        store.create_employee(&creation("new-key", "rhea")).unwrap_err(),
        StoreError::EmployeeConflict { .. }
    ));
}
```

- [x] **Step 2: Run and verify RED**

Run: `cargo test -p lazoo-store --test store --test employee`

Expected: FAIL because migration 2 and employee persistence are absent.

- [x] **Step 3: Implement ordered migrations and create/list/show**

Refactor migration application to validate and apply `[(1, 0001), (2, 0002)]` in order. Add employee, provider-observation, and roster-state tables. Implement atomic create with processed-command replay, audit event, outbox row, and global roster revision.

- [x] **Step 4: Run focused and workspace store tests**

Run: `cargo test -p lazoo-store --test store --test employee`

Expected: PASS with schema version 2 and zero failures.

- [x] **Step 5: Commit**

```powershell
git add migrations crates/lazoo-store
git commit -m "feat: persist employee identities"
```

### Task 3: Revision-checked update, disable, and roster state

**Files:**
- Modify: `crates/lazoo-store/src/employee.rs`
- Modify: `crates/lazoo-store/src/lib.rs`
- Modify: `crates/lazoo-store/tests/employee.rs`

- [x] **Step 1: Write failing revision tests**

```rust
#[test]
fn update_requires_current_revision_and_noop_does_not_advance_roster() {
    let created = store.create_employee(&creation("create", "Rhea")).unwrap();
    let changed = store.update_employee(&update(created.employee.id, 1, "Lead")).unwrap();
    assert_eq!(changed.employee.configuration_revision, 2);
    assert_eq!(changed.roster_revision, 2);
    let replayed_noop = store.update_employee(&update_noop(changed.employee.clone())).unwrap();
    assert_eq!(replayed_noop.roster_revision, 2);
    assert!(replayed_noop.event_sequence.is_none());
}

#[test]
fn disable_preserves_record_and_rejects_stale_revision() {
    let created = store.create_employee(&creation("create", "Rhea")).unwrap();
    store.disable_employee(&disable(created.employee.id, 1)).unwrap();
    assert!(!store.get_employee(created.employee.id).unwrap().enabled);
    assert!(matches!(
        store.disable_employee(&disable(created.employee.id, 1)).unwrap_err(),
        StoreError::EmployeeStale { current_revision: 2 }
    ));
}
```

- [x] **Step 2: Run and verify RED**

Run: `cargo test -p lazoo-store --test employee`

Expected: FAIL on missing update/disable behavior.

- [x] **Step 3: Implement revision-checked mutations**

Apply patches only after expected-revision validation; compare effective configuration before incrementing revisions; retain disabled rows and timestamps; replay idempotent responses exactly.

- [x] **Step 4: Run and verify GREEN**

Run: `cargo test -p lazoo-store --test employee`

Expected: PASS with no duplicate events or revision inflation.

- [x] **Step 5: Commit**

```powershell
git add crates/lazoo-store
git commit -m "feat: revision employee configuration"
```

### Task 4: Provider observations and deterministic availability

**Files:**
- Create: `crates/lazoo-domain/src/roster.rs`
- Modify: `crates/lazoo-domain/src/lib.rs`
- Test: `crates/lazoo-domain/tests/roster.rs`
- Modify: `crates/lazoo-store/src/employee.rs`
- Modify: `crates/lazoo-store/tests/employee.rs`

- [x] **Step 1: Write failing availability and refresh tests**

```rust
#[test]
fn availability_requires_session_capability_and_ready_authentication() {
    assert_eq!(derive_availability(false, &configured_probe()), EmployeeAvailability::Disabled);
    assert_eq!(derive_availability(true, &missing_probe()), EmployeeAvailability::Offline);
    assert_eq!(derive_availability(true, &unverified_probe()), EmployeeAvailability::Unverified);
    assert_eq!(derive_availability(true, &configured_probe()), EmployeeAvailability::Dormant);
}

#[test]
fn observation_timestamp_only_refresh_does_not_advance_roster_revision() {
    store.refresh_provider_observations(&refresh("one", vec![observation("t1")])).unwrap();
    let revision = store.roster_snapshot().unwrap().revision;
    store.refresh_provider_observations(&refresh("two", vec![observation("t2")])).unwrap();
    assert_eq!(store.roster_snapshot().unwrap().revision, revision);
}
```

- [x] **Step 2: Run and verify RED**

Run: `cargo test -p lazoo-domain --test roster && cargo test -p lazoo-store --test employee`

Expected: FAIL because availability and observation refresh are absent.

- [x] **Step 3: Implement safe observations and roster derivation**

Persist normalized probe fields without executable paths or raw evidence. Compare effective fields excluding `checkedAt`; increment roster revision and emit `roster.refreshed` only when effective employee-facing state changes.

- [x] **Step 4: Run and verify GREEN**

Run: `cargo test -p lazoo-domain --test roster && cargo test -p lazoo-store --test employee`

Expected: PASS with deterministic state mapping.

- [x] **Step 5: Commit**

```powershell
git add crates/lazoo-domain crates/lazoo-store
git commit -m "feat: derive live employee roster"
```

### Task 5: Daemon and CLI employee workflow

**Files:**
- Modify: `apps/lazood/src/app.rs`
- Modify: `apps/lazood/tests/daemon.rs`
- Modify: `apps/lazooctl/src/lib.rs`
- Modify: `apps/lazooctl/tests/cli.rs`
- Modify: `README.md`

- [x] **Step 1: Write failing RPC and CLI tests**

```rust
#[tokio::test]
async fn employee_create_list_refresh_disable_is_durable_and_ui_neutral() {
    let created = app.handle(request("create", "employee.create", json!({
        "name":"Rhea","provider":"codex","idempotencyKey":"create-rhea"
    }))).await.result.unwrap();
    assert_eq!(created["employee"]["name"], "Rhea");
    let roster = app.handle(request("roster", "roster.get", json!({}))).await.result.unwrap();
    assert_eq!(roster["revision"], 1);
    let desktop = app.handle(request("desktop", "desktop.snapshot", json!({}))).await.result.unwrap();
    assert!(desktop.get("employees").is_none());
}

#[test]
fn employee_cli_maps_mutations_and_roster_refresh() {
    let create = Cli::try_parse_from(["lazooctl","employee","create","Rhea","--provider","codex"]).unwrap();
    assert_eq!(command_request(&create.command).0, "employee.create");
    let refresh = Cli::try_parse_from(["lazooctl","roster","refresh"]).unwrap();
    assert_eq!(command_request(&refresh.command).0, "roster.refresh");
}
```

- [x] **Step 2: Run and verify RED**

Run: `cargo test -p lazood -p lazooctl`

Expected: FAIL because employee/roster methods and CLI commands are missing.

- [x] **Step 3: Implement daemon probe orchestration and CLI mapping**

Inject a discovery environment seam into `App`; probe only during create, provider-changing update, and explicit refresh; deduplicate enabled providers on refresh; map store/domain errors to stable employee codes and CLI exit code 9.

- [x] **Step 4: Run focused tests and verify GREEN**

Run: `cargo test -p lazood -p lazooctl`

Expected: PASS with `desktop.snapshot` unchanged.

- [x] **Step 5: Commit**

```powershell
git add apps/lazood apps/lazooctl README.md
git commit -m "feat: expose employee roster controls"
```

### Task 6: Acceptance and full verification

**Files:**
- Create: `scripts/acceptance/employee-roster.ps1`
- Create: `docs/verification/2026-08-30-employee-roster.md`
- Modify: `scripts/acceptance/first-slice.ps1`
- Modify: `docs/NEXT-SESSION.md`
- Modify: `docs/PROJECT-BRIEF.md`

- [x] **Step 1: Write the acceptance contract test first**

Extend Rust acceptance-contract tests to require isolated data roots, create replay, restart durability, stale update rejection, monotonic roster revisions, preserved disabled identity, provider-smoke absence, and unchanged desktop capability scope.

- [x] **Step 2: Run the focused contract test and verify RED**

Run: `cargo test -p lazood --test daemon employee_roster_acceptance_contract_is_backend_only`

Expected: FAIL because the acceptance script is absent.

- [x] **Step 3: Implement the acceptance harness and synchronize schema expectations**

Use release `lazood` and `lazooctl` against a temporary data root. Update the first-slice database-schema expectation from 1 to 2 while retaining protocol version 1. Leave the independent manifest-format versions in the desktop and provider acceptance scripts unchanged.

- [x] **Step 4: Run full verification**

```powershell
& .\scripts\verify-rust-toolchain.ps1
cargo test --workspace
cargo fmt --all -- --check
cargo clippy --workspace --all-targets -- -D warnings
cargo check --workspace --all-targets
cargo build --workspace --release --locked
powershell -NoProfile -ExecutionPolicy Bypass -File scripts\acceptance\first-slice.ps1 -SkipBuild
powershell -NoProfile -ExecutionPolicy Bypass -File scripts\acceptance\employee-roster.ps1 -SkipBuild
git diff --check
```

Expected: every command exits 0. Live model tests remain explicit and ignored in the ordinary workspace suite.

- [x] **Step 5: Record evidence and commit**

Write exact counts, exit codes, schema/protocol versions, event/revision evidence, and exclusions in `docs/verification/2026-08-30-employee-roster.md`; update continuity docs without claiming terminals, tasks, approvals, or UI.

```powershell
git add scripts docs artifacts/acceptance
git commit -m "test: verify durable employee roster"
```
