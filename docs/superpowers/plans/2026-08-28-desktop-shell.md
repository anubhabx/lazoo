# Lazoo Desktop Shell Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Deliver a secure, read-only Tauri 2 Windows shell that presents one typed snapshot of Lazoo daemon health, projects, recent events, and incident count.

**Architecture:** Add one additive `desktop.snapshot` protocol method and return type, then expose it through a small Tauri Rust bridge whose only webview command is `load_desktop_snapshot`. Serve a checked-in vanilla HTML/CSS/ES-module interface directly through Tauri, with pure view formatting tested by Node and real-window acceptance exercised against an isolated keeper/daemon.

**Tech Stack:** Rust 1.98, Tokio, existing Lazoo JSON-RPC/named-pipe crates, Tauri 2.11, WebView2, static HTML/CSS/JavaScript, Node 24 built-in test runner, PowerShell acceptance.

**Execution status:** Complete on 2026-08-28. All five tasks were executed with inline red-green-refactor cycles; exact gate evidence is recorded in `docs/verification/2026-08-28-desktop-shell.md`.

---

## File map

- `crates/lazoo-protocol/src/types.rs` owns the shared `DesktopSnapshot` DTO.
- `crates/lazoo-protocol/tests/contracts.rs` proves strict camel-case wire behavior.
- `apps/lazood/src/app.rs` assembles the read-only snapshot.
- `apps/lazood/tests/daemon.rs` proves the new RPC contract with real store state.
- `apps/lazoo-desktop/src/client.rs` owns one-shot authenticated IPC and public error mapping.
- `apps/lazoo-desktop/src/lib.rs` registers the sole Tauri command and application builder.
- `apps/lazoo-desktop/src/main.rs` is the native executable entry point.
- `apps/lazoo-desktop/ui/view-model.js` contains pure presentation transformations.
- `apps/lazoo-desktop/ui/app.js` owns refresh state and safe DOM rendering.
- `apps/lazoo-desktop/ui/styles.css` owns the responsive visual system.
- `apps/lazoo-desktop/ui/index.html` provides the semantic shell.
- `apps/lazoo-desktop/tauri.conf.json`, `build.rs`, and `capabilities/main.json` define the Tauri security/build boundary.
- `scripts/acceptance/desktop-shell.ps1` proves release build, isolated daemon connectivity, and a real Windows main window.
- `docs/verification/2026-08-28-desktop-shell.md`, `README.md`, and `docs/PROJECT-BRIEF.md` synchronize shipped status.

### Task 1: Typed desktop snapshot RPC

**Files:**
- Modify: `crates/lazoo-protocol/src/types.rs`
- Modify: `crates/lazoo-protocol/src/lib.rs`
- Modify: `crates/lazoo-protocol/tests/contracts.rs`
- Modify: `apps/lazood/src/app.rs`
- Modify: `apps/lazood/tests/daemon.rs`

- [x] **Step 1: Write the failing protocol serialization test**

Add a test that constructs a `DesktopSnapshot`, serializes it, asserts `capturedAt`, `status`, `projects`, and `events`, and verifies an unknown field is rejected during deserialization.

```rust
#[test]
fn desktop_snapshot_uses_strict_camel_case_contract() {
    let snapshot = DesktopSnapshot {
        captured_at: "2026-08-28T00:00:00Z".to_owned(),
        status: sample_status(),
        projects: Vec::new(),
        events: Vec::new(),
    };
    let value = serde_json::to_value(&snapshot).expect("snapshot serializes");
    assert_eq!(value["capturedAt"], "2026-08-28T00:00:00Z");
    let mut invalid = value;
    invalid["unexpected"] = json!(true);
    assert!(serde_json::from_value::<DesktopSnapshot>(invalid).is_err());
}
```

- [x] **Step 2: Run the test and verify RED**

Run: `cargo test -p lazoo-protocol --test contracts desktop_snapshot_uses_strict_camel_case_contract`

Expected: compilation fails because `DesktopSnapshot` does not exist.

- [x] **Step 3: Add the minimal shared DTO**

```rust
#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
pub struct DesktopSnapshot {
    pub captured_at: String,
    pub status: SystemStatus,
    pub projects: Vec<ProjectRecord>,
    pub events: Vec<EventSummary>,
}
```

Re-export it from `lazoo-protocol` and rerun the focused test to verify GREEN.

- [x] **Step 4: Write failing daemon snapshot tests**

Add one test that registers a real temporary Git repository, calls `desktop.snapshot`, decodes `DesktopSnapshot`, and asserts the project and event are present. Add another request with a nonempty params object and assert `LZ-INVALID-PARAMS`.

- [x] **Step 5: Run the daemon tests and verify RED**

Run: `cargo test -p lazood --test daemon desktop_snapshot`

Expected: `desktop.snapshot` returns `LZ-METHOD-NOT-FOUND`.

- [x] **Step 6: Implement snapshot assembly**

Add a strict empty-params decoder and a method that reads store status once, uses `event_high_water.saturating_sub(100)` as the recent cursor, and returns:

```rust
DesktopSnapshot {
    captured_at: Utc::now().to_rfc3339(),
    status: self.system_status(store_status)?,
    projects: self.store.list_projects().map_err(store_error)?,
    events: self.store.list_events(recent_after, 100).map_err(store_error)?,
}
```

Keep `system.status` behavior unchanged by sharing only the internal status assembly.

- [x] **Step 7: Verify GREEN and commit**

Run: `cargo test -p lazoo-protocol -p lazood`

Expected: all focused tests pass.

Commit: `feat: add desktop snapshot RPC`

### Task 2: Test-first Tauri bridge and security scaffold

**Files:**
- Create: `apps/lazoo-desktop/Cargo.toml`
- Create: `apps/lazoo-desktop/build.rs`
- Create: `apps/lazoo-desktop/src/client.rs`
- Create: `apps/lazoo-desktop/src/lib.rs`
- Create: `apps/lazoo-desktop/src/main.rs`
- Create: `apps/lazoo-desktop/tauri.conf.json`
- Create: `apps/lazoo-desktop/capabilities/main.json`
- Create: `apps/lazoo-desktop/tests/config.rs`
- Modify: `Cargo.toml`

- [x] **Step 1: Add only the crate test surface and failing client tests**

Create the crate with `serde`, `serde_json`, `thiserror`, `tokio`, `uuid`, `lazoo-ipc`, `lazoo-protocol`, `lazoo-runtime`, `tauri = "2.11.5"`, and `tauri-build = "2.6.3"`. Register it in the workspace. In `client.rs`, write tests for the wished-for API:

```rust
#[test]
fn snapshot_request_cannot_select_an_arbitrary_method() {
    let request = snapshot_request("desktop-test");
    assert_eq!(request.method, "desktop.snapshot");
    assert_eq!(request.params, json!({}));
}

#[test]
fn rpc_failure_preserves_public_code_and_message() {
    let error = parse_snapshot_response(JsonRpcResponse::failure(
        "desktop-test",
        RpcError { code: "LZ-STORE".into(), message: "store unavailable".into(), data: None },
    ))
    .expect_err("RPC error must fail");
    assert_eq!(DesktopFailure::from(&error).code, "LZ-STORE");
}

#[test]
fn invalid_response_is_redacted() {
    let error = parse_snapshot_response(JsonRpcResponse {
        jsonrpc: "2.0".into(), id: "desktop-test".into(), result: None, error: None,
    })
    .expect_err("invalid shape must fail");
    let public = DesktopFailure::from(&error);
    assert_eq!(public.code, "LZ-DESKTOP-INVALID-RESPONSE");
    assert!(!public.message.contains("nonce"));
}
```

- [x] **Step 2: Run the focused tests and verify RED**

Run: `cargo test -p lazoo-desktop --lib client::tests`

Expected: compilation fails because the client functions and types are absent.

- [x] **Step 3: Implement the minimal typed bridge**

Implement `snapshot_request`, `parse_snapshot_response`, public failure mapping, and:

```rust
async fn fetch_snapshot_inner() -> Result<DesktopSnapshot, DesktopClientError> {
    let root = DataRoot::resolve().map_err(DesktopClientError::from)?;
    let installation = Installation::load_or_create(&root).map_err(DesktopClientError::from)?;
    let mut connection = PipeClient::connect(
        &installation,
        "lazoo-desktop",
        env!("CARGO_PKG_VERSION"),
    )
    .await
    .map_err(DesktopClientError::from)?;
    let request = snapshot_request(Uuid::new_v4().to_string());
    connection.send_json(&request).await.map_err(DesktopClientError::from)?;
    let response = connection.receive_json().await.map_err(DesktopClientError::from)?;
    parse_snapshot_response(response)
}

pub async fn fetch_snapshot() -> Result<DesktopSnapshot, DesktopFailure> {
    fetch_snapshot_inner()
        .await
        .map_err(|error| DesktopFailure::from(&error))
}
```

Ensure all internal errors become `DesktopFailure { code, message }` without debug formatting or secret-bearing data.

- [x] **Step 4: Verify bridge GREEN**

Run: `cargo test -p lazoo-desktop --lib client::tests`

Expected: all client tests pass.

- [x] **Step 5: Write failing Tauri configuration tests**

The tests must parse `tauri.conf.json` and assert:

- product name `Lazoo` and identifier `com.anubhab.lazoo`;
- `build.frontendDist` equals `ui`;
- `app.withGlobalTauri` is true;
- exactly one main window exists and its minimum size is 860 by 620;
- CSP exists and `dangerousDisableAssetCspModification` is absent;
- bundle creation is disabled for this slice;
- the capability contains only `core:default` and generated `allow-load-desktop-snapshot` permissions.

- [x] **Step 6: Run configuration tests and verify RED**

Run: `cargo test -p lazoo-desktop --test config`

Expected: configuration files are missing.

- [x] **Step 7: Add the Tauri builder and restricted configuration**

Register only:

```rust
#[tauri::command]
async fn load_desktop_snapshot() -> Result<DesktopSnapshot, DesktopFailure> {
    client::fetch_snapshot().await
}
```

Use `tauri::generate_handler![load_desktop_snapshot]`. Configure one decorated, resizable main window, static `ui` frontend, `withGlobalTauri`, local-only CSP, and `bundle.active: false`. Use the documented Tauri build manifest to declare only `load_desktop_snapshot`.

- [x] **Step 8: Verify configuration and native compilation, then commit**

Run: `cargo test -p lazoo-desktop`

Run: `cargo check -p lazoo-desktop --all-targets`

Expected: both exit 0 without warnings.

Commit: `feat: add secure Lazoo Tauri bridge`

### Task 3: Tested presentation model

**Files:**
- Create: `apps/lazoo-desktop/ui/view-model.js`
- Create: `apps/lazoo-desktop/ui/view-model.test.mjs`

- [x] **Step 1: Write failing Node tests**

Cover exact behavior:

```javascript
test('formats daemon uptime without sub-minute noise', () => {
  assert.equal(formatUptime(59_999), '< 1 min');
  assert.equal(formatUptime(3_660_000), '1 hr 1 min');
});

test('builds degraded health when the daemon reports reasons', () => {
  const model = buildViewModel(snapshot({ degraded: ['outbox delayed'] }));
  assert.equal(model.health.label, 'Attention needed');
  assert.equal(model.health.tone, 'warning');
});

test('summarizes project registration without rendering raw JSON', () => {
  assert.equal(
    summarizeEvent({ eventType: 'project.registered', payload: { displayName: 'Orchestrator' } }),
    'Orchestrator joined the zoo',
  );
});
```

Also cover healthy state, recovered incidents, checkout labels, invalid timestamps, unknown event types, and long payload truncation.

- [x] **Step 2: Run Node tests and verify RED**

Run: `node --test apps/lazoo-desktop/ui/view-model.test.mjs`

Expected: module-not-found or missing-export failure.

- [x] **Step 3: Implement pure formatting functions**

Export `formatUptime`, `formatTimestamp`, `formatRelativeTime`, `formatCheckoutMode`, `humanizeEventType`, `summarizeEvent`, and `buildViewModel`. Do not access `window` or `document` in this module.

- [x] **Step 4: Verify GREEN and commit**

Run: `node --test apps/lazoo-desktop/ui/view-model.test.mjs`

Expected: all JavaScript tests pass.

Commit: `feat: add desktop presentation model`

### Task 4: Native control-room interface

**Files:**
- Create: `apps/lazoo-desktop/ui/index.html`
- Create: `apps/lazoo-desktop/ui/styles.css`
- Create: `apps/lazoo-desktop/ui/app.js`

- [x] **Step 1: Build the semantic static document**

Use a real `header`, `main`, `section`, `ol`, `dl`, and `button`. Include one polite live region for connection state. Load `styles.css` and `app.js` as local files; include no inline style, inline script, remote resource, or form.

- [x] **Step 2: Implement serialized refresh and safe rendering**

`app.js` must:

- call `window.__TAURI__.core.invoke('load_desktop_snapshot')`;
- ignore concurrent refresh requests;
- preserve the last snapshot on failure;
- render all daemon values through `textContent` and DOM creation;
- refresh every ten seconds only when `document.visibilityState === 'visible'`;
- expose connected, degraded, disconnected, loading, project-empty, and activity-empty states;
- make the manual refresh button keyboard accessible and reflect busy state.

- [x] **Step 3: Apply the responsive visual system**

Use the spec's paper/ink/orange/green palette, Bahnschrift/Segoe typography, asymmetric primary/diagnostic grid, thin rules, visible focus, reduced-motion handling, and a single-column narrow layout. Avoid external assets, gradients, glow, glass, nested cards, and decorative charts.

- [x] **Step 4: Run static safety checks and the frontend tests**

Run: `rg -n "innerHTML|outerHTML|insertAdjacentHTML|https?://|<script[^>]*>[^<]" apps/lazoo-desktop/ui`

Expected: no unsafe DOM sink or remote resource matches.

Run: `node --test apps/lazoo-desktop/ui/view-model.test.mjs`

Expected: all tests pass.

- [x] **Step 5: Compile the embedded frontend and commit**

Run: `cargo build -p lazoo-desktop`

Expected: `target/debug/lazoo-desktop.exe` exists.

Commit: `feat: build Lazoo desktop control room`

### Task 5: Windows acceptance, visual QA, and contract synchronization

**Files:**
- Create: `scripts/acceptance/desktop-shell.ps1`
- Create: `artifacts/acceptance/desktop-shell-manifest.json`
- Create: `docs/verification/2026-08-28-desktop-shell.md`
- Modify: `README.md`
- Modify: `docs/PROJECT-BRIEF.md`
- Modify: `docs/superpowers/plans/2026-08-28-desktop-shell.md`
- Modify: `docs/superpowers/specs/2026-08-28-desktop-shell-design.md`

- [x] **Step 1: Write the acceptance script assertions before relying on the new app**

The script must use a unique temporary `LAZOO_DATA_ROOT`, build locked release binaries, initialize a temporary Git repository, start `lazoo-keeper.exe`, register the project, launch `lazoo-desktop.exe`, wait for a nonzero `MainWindowHandle` and exact title, capture default and narrow temporary screenshots, stop only the exact desktop/keeper processes it started, retain the isolated run directory for visual evidence, and write a JSON manifest.

Required manifest gates:

```json
{
  "releaseBuild": "PASS",
  "daemonReady": "PASS",
  "snapshotRpc": "PASS",
  "desktopWindow": "PASS",
  "defaultScreenshot": "PASS",
  "narrowScreenshot": "PASS",
  "processIsolation": "PASS"
}
```

- [x] **Step 2: Run acceptance and inspect screenshots**

Run: `powershell -NoProfile -ExecutionPolicy Bypass -File scripts\acceptance\desktop-shell.ps1`

Expected: `DESKTOP SHELL ACCEPTANCE: PASS` and manifest status `PASS`.

Inspect both screenshots for clipping, hierarchy, focus visibility, narrow-layout reflow, correct project content, and a readable system pulse. Record exact observations in the verification file. Delete only the temporary screenshots after inspection.

- [x] **Step 3: Synchronize product documentation**

Update `README.md` to list four executables and exact desktop run/acceptance commands. Update the project brief's repository stage, first-slice exclusions, Stage 2 status, and next gate without implying employee/provider support. Mark the design Implemented only after all required gates pass.

- [x] **Step 4: Run the full fresh gate sequence**

Run in order:

```powershell
powershell -NoProfile -ExecutionPolicy Bypass -File scripts\verify-rust-toolchain.ps1
node --test apps\lazoo-desktop\ui\view-model.test.mjs
cargo test --workspace
cargo fmt --all -- --check
cargo clippy --workspace --all-targets -- -D warnings
cargo check --workspace --all-targets
cargo build --workspace --release --locked
powershell -NoProfile -ExecutionPolicy Bypass -File scripts\acceptance\first-slice.ps1
powershell -NoProfile -ExecutionPolicy Bypass -File scripts\acceptance\desktop-shell.ps1 -SkipBuild
git diff --check
```

Record exact test counts, exit results, acceptance manifest gates, manual evidence, and any blocker in `docs/verification/2026-08-28-desktop-shell.md`.

- [x] **Step 5: Final contract review and commit**

Compare the implementation line by line against `docs/superpowers/specs/2026-08-28-desktop-shell-design.md`. Confirm every exclusion remains excluded and no gate is claimed without output.

Commit: `docs: verify Lazoo desktop shell slice`
