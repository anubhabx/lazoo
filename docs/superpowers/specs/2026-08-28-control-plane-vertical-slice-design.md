# Lazoo Control-Plane Vertical Slice Design

- **Status:** Implemented and verified
- **Date:** 2026-08-28
- **Scope:** First runnable Windows control-plane milestone
- **Parent contract:** docs/PROJECT-BRIEF.md

## 1. Outcome

This milestone proves that Lazoo can run as a small, recoverable Windows control plane before provider adapters or a desktop interface exist.

The completed slice will:

1. start lazoo-keeper.exe for the current Windows user;
2. have the keeper start and supervise lazood.exe;
3. connect lazooctl.exe to the daemon through a secured Windows named pipe;
4. register and list a local Git project through a versioned control protocol;
5. persist project state, audit events, and outbox events in SQLite WAL;
6. survive a daemon restart without losing the registered project or event sequence;
7. stop a restart loop after a bounded budget;
8. record a daemon-unavailable incident through an append-only emergency spool when the daemon cannot report its own failure; and
9. expose exact status and diagnostic evidence through the CLI.

This is a production-shaped walking skeleton, not a throwaway prototype. Its boundaries are intentionally narrow so later slices can extend stable seams rather than replace a monolith.

## 2. Non-goals

The milestone does not include:

- the Tauri desktop shell or tray;
- provider discovery or adapters;
- employees, conversations, turns, tasks, or peer messaging;
- ConPTY or native terminal attachment;
- provider or employee Job Objects;
- worktree creation, branch management, or integration;
- policy evaluation or approval cards;
- managed memory processing;
- MCP, plugin, skill, or connector federation;
- installers, signed updates, or background auto-start;
- a network listener; or
- macOS or Linux support.

The keeper supervises only the daemon in this slice. General process-tree governance remains a later control-plane-kernel responsibility.

## 3. Selected approach

The selected approach is an end-to-end vertical slice. Only contracts exercised by the slice are defined.

Two alternatives were rejected:

- **Contracts first:** defining every future domain and provider schema before any executable path would delay Windows and recovery feedback and encourage speculative abstractions.
- **Desktop first:** adding Tauri now would introduce webview, capability, packaging, and UI-state concerns before the daemon contract is proven.

The slice keeps future compatibility through versioned protocol envelopes, explicit module boundaries, migrations, and dependency inversion at operating-system boundaries.

## 4. Architecture

The repository becomes one Rust workspace with three binaries and focused library crates:

| Unit | Responsibility | Depends on |
|---|---|---|
| lazoo-protocol | Versioned request, response, event, error, identifier, and handshake types | Serialization and validation only |
| lazoo-domain | Project-registration rules, checkout-mode values, domain commands, and domain events | lazoo-protocol types where shared at the boundary |
| lazoo-store | SQLite migrations, transactions, project repository, audit log, outbox, event reads, and recovery checkpoints | lazoo-domain |
| lazoo-ipc | Named-pipe server/client, framing, current-user authorization, nonce handshake, and sequence-aware event delivery | lazoo-protocol plus Windows bindings |
| lazoo-runtime | Data-directory resolution, atomic runtime files, structured logging, clock and identifier ports | Small platform abstractions |
| lazood.exe | Composition root and authoritative command handler | all library crates |
| lazooctl.exe | Human and machine-readable CLI client | lazoo-protocol, lazoo-ipc, lazoo-runtime |
| lazoo-keeper.exe | Daemon supervision, restart budget, emergency spool, and native notification boundary | lazoo-runtime and narrow Windows bindings |

The domain crate does not depend on SQLite, named pipes, process APIs, or CLI parsing. The store does not start processes. The IPC layer does not make domain decisions. Each binary is a thin composition root.

The exact Rust runtime, SQLite library, CLI parser, serialization library, and Windows binding crates are selected in the implementation plan after current documentation and compatibility checks. The design does not require an ORM.

## 5. Repository shape

The implementation plan should create this structure:

    Cargo.toml
    rust-toolchain.toml
    crates/
      lazoo-protocol/
      lazoo-domain/
      lazoo-store/
      lazoo-ipc/
      lazoo-runtime/
    apps/
      lazood/
      lazooctl/
      lazoo-keeper/
    migrations/
    tests/
      windows/
      fixtures/
    scripts/
      acceptance/
    docs/
      adr/

The workspace starts with one dependency-policy file and one formatting/lint configuration. It does not add a JavaScript toolchain because no webview code exists in this slice.

## 6. Local data layout

All mutable state lives outside Git under the current user's local application data:

    %LOCALAPPDATA%\Lazoo\
      state\lazoo.db
      runtime\install.json
      runtime\keeper-state.json
      spool\active.open
      spool\pending\
      spool\reconciled\
      logs\lazood\
      logs\keeper\

install.json contains a random installation identifier, protocol bootstrap metadata, and a protected reference to the local handshake nonce. The nonce itself is protected for the current user with DPAPI or an equivalently scoped Windows credential facility selected in the implementation plan.

Runtime and spool files are created with explicit current-user access control. Paths are canonicalized before use. Test builds accept an explicit isolated data root so tests never touch the owner's real Lazoo state.

## 7. Control protocol

### 7.1 Transport

The control channel is a Windows named pipe with:

- an unpredictable installation-scoped pipe name;
- PIPE_REJECT_REMOTE_CLIENTS or its Windows-binding equivalent;
- an explicit discretionary access control list granting full control only to the installation owner's SID and LocalSystem, with no grant to Administrators, Authenticated Users, Users, Anonymous, or Network;
- JSON-RPC 2.0 messages in UTF-8 JSON, framed by a four-byte little-endian unsigned payload length;
- a one-mebibyte maximum frame and a 1,024-event outbound queue per client;
- independent read and write servicing;
- clean disconnection of a client whose queue remains saturated, with its last acknowledged event sequence retained by that client for reconnect; and
- no TCP fallback.

Terminal traffic is not part of this slice and therefore cannot share or starve the control channel.

### 7.2 Authentication and initialization

Pipe access requires both:

1. a client token whose Windows SID matches the installation owner, verified from the impersonated named-pipe client token; and
2. proof of the per-installation local nonce during initialization.

After pipe connection, the server sends a minimal unauthenticated hello containing a random per-connection challenge and its supported protocol range. The first client request is initialize. It carries:

- supported protocol versions;
- client name and version;
- installation identifier;
- an HMAC-SHA-256 proof keyed by the protected installation nonce, over length-prefixed UTF-8 fields in this fixed order: server challenge, installation identifier, client name, client version, and offered protocol versions;
- requested event sequence; and
- desired output capabilities.

The daemon returns the selected protocol version, daemon version, boot identifier, current event high-water mark, and capability flags. The raw nonce is never sent over the pipe or written to logs. A SID, proof, installation, or version mismatch fails closed with a stable error and no domain command execution.

### 7.3 Slice methods

The initial method set is deliberately small:

| Method | Purpose |
|---|---|
| system.status | Return daemon, keeper-observation, store, protocol, and event high-water status |
| project.register | Register a canonical local Git repository and its checkout mode |
| project.list | List registered projects and their current configuration revision |
| events.list | Return persisted public audit/event summaries after a monotonic sequence |
| events.subscribe | Stream new public event summaries after a supplied monotonic sequence, beginning with persisted catch-up |
| system.shutdown | Request a planned daemon and keeper shutdown for tests and explicit local administration |

All mutations include a request identifier and idempotency key. Unknown fields are rejected for commands in the first protocol version so accidental client/server drift is visible.

## 8. First durable domain flow

project.register is the first end-to-end mutation.

Input:

- project path;
- optional display name;
- checkout mode, defaulting to all-editors-isolated;
- request identifier; and
- idempotency key.

Validation:

1. canonicalize the path without following it into an unsafe cleanup or managed-state root;
2. require an existing local Git worktree or repository;
3. resolve the common Git directory and current checkout;
4. reject a path already registered under a conflicting identity;
5. validate the checkout-mode enum; and
6. reject mutation if the daemon is in read-only recovery mode.

One SQLite transaction:

1. inserts or returns the idempotent project record;
2. records the selected checkout mode and configuration revision;
3. appends an immutable ProjectRegistered audit event; and
4. appends the corresponding outbox record.

Only after commit does the daemon acknowledge success. The response includes the project identifier, canonical path, checkout mode, configuration revision, and event sequence.

The outbox dispatcher in this slice marks the internal event published to the daemon event stream. It does not deliver to providers or external systems. Keeping the outbox in the first slice proves the atomic state-plus-event contract before the task bus depends on it.

## 9. Storage model

The first migration contains:

- schema_migrations;
- installations;
- projects;
- processed_commands;
- audit_events;
- outbox;
- recovery_runs; and
- emergency_incident_imports.

Required properties:

- SQLite WAL mode;
- foreign keys enabled;
- one authoritative daemon writer;
- explicit busy timeout;
- monotonic audit sequence allocated transactionally;
- immutable audit-event payload after insertion;
- unique idempotency key per command scope;
- outbox rows linked to their audit event;
- UTC timestamps plus monotonic process timing for local deadlines; and
- forward-only, checksum-verified migrations.

If a migration is unknown, changed after application, or fails, the daemon does not serve mutating requests. It records the condition when safe, reports degraded read-only status if possible, and exits when safe reads cannot be guaranteed.

## 10. Daemon lifecycle

Startup order:

1. resolve and secure the local data directories;
2. acquire the single-daemon installation lock;
3. initialize structured logging with secret redaction;
4. open SQLite and validate/apply known migrations;
5. reconcile emergency-spool records;
6. replay pending internal outbox rows;
7. create the secured named pipe;
8. emit DaemonReady with a new boot identifier; and
9. accept clients.

Shutdown order:

1. stop accepting new mutating commands;
2. drain bounded in-flight handlers;
3. persist a shutdown checkpoint;
4. close pipe clients;
5. checkpoint WAL when safe;
6. emit final structured diagnostics; and
7. exit with a documented code.

A successful system.shutdown uses the documented planned-stop exit code. The keeper recognizes that code, records a clean stop, does not restart the daemon, and exits itself. Every other unrequested daemon termination enters the restart-budget flow.

The daemon never deletes a database, spool, log, or registered project to recover from startup failure.

## 11. Keeper and dual-path failsafe

The keeper is intentionally independent from SQLite and ordinary daemon command handling. It acquires a single-keeper installation lock, owns the daemon child process, and observes:

- process start success;
- exit code and runtime;
- consecutive unexpected exits;
- restart-budget state.

The keeper publishes only non-secret observations through runtime\keeper-state.json using atomic replacement. The daemon may read this file for system.status, but correctness and restart decisions never depend on the daemon reading or acknowledging it.

The default slice budget is three unexpected daemon exits in five minutes. A daemon that remains healthy for ten minutes resets the consecutive-failure window. These values are constants with tests in this slice and become configurable in a later settings slice.

On an unexpected exit, the keeper:

1. records the occurrence in its small atomic state file;
2. restarts the daemon when budget remains;
3. applies bounded backoff of one, two, then five seconds;
4. stops restarting when the budget is exhausted;
5. appends one deduplicated LZ-DAEMON-UNAVAILABLE record to the active emergency segment; and
6. invokes the Windows notification boundary.

The spool write is append-only, newline-delimited JSON, flushed to disk, and independent of SQLite. Each record is limited to 16 KiB, each segment to 1 MiB, and unreconciled emergency storage to eight segments. The keeper writes active.open and seals it into an immutable, uniquely named pending segment using atomic rename before exit or rotation. On startup it validates and seals a stale active.open left by a prior keeper crash. The daemon imports only sealed pending segments.

If all eight unreconciled segments are full, the keeper preserves existing evidence, writes the new failure to the Windows event-log boundary when available, raises the native notification, and emits a last-resort stderr diagnostic; it never silently overwrites an older unreconciled incident. After restart-budget exhaustion, the keeper seals the active spool, releases its installation lock, and exits nonzero so a later explicit keeper start can attempt recovery cleanly.

On recovery, the daemon imports each emergency record exactly once using its incident identifier, records the import transactionally, and moves a fully reconciled spool segment into the reconciled directory. A partially written final line remains for the next recovery attempt and is never treated as a valid record.

The notification implementation may require installer identity for the final production path. The slice therefore requires a real Windows notification on the baseline machine and a replaceable notifier port for deterministic automated tests. Failure to display the notification must not prevent the spool record.

## 12. CLI experience

The CLI supports concise human output by default and stable JSON with --json.

Initial commands:

    lazooctl status
    lazooctl project register <path> [--name <name>] [--checkout-mode isolated|lead-main]
    lazooctl project list
    lazooctl events list [--after <sequence>] [--limit <count>]
    lazooctl events follow [--after <sequence>]
    lazooctl system shutdown

Exit codes distinguish:

- success;
- invalid user input;
- daemon unavailable;
- protocol mismatch;
- authorization failure;
- domain conflict;
- degraded/read-only store; and
- internal failure.

Errors show a stable code, plain-language explanation, and one next action. Machine output contains the same code and structured details. Secrets, nonce material, raw security descriptors, and unrelated paths are never printed.

## 13. Error handling and recovery

The slice uses stable error families:

| Code | Meaning | Required behavior |
|---|---|---|
| LZ-IPC-AUTH | SID or nonce proof failed | Close connection; do not execute or reveal state |
| LZ-PROTOCOL-INCOMPATIBLE | No compatible protocol version | Return supported range and stop initialization |
| LZ-DAEMON-ALREADY-RUNNING | Installation lock is held | Exit without touching the active daemon |
| LZ-STORE-MIGRATION | Store cannot be migrated safely | Refuse mutations; retain evidence |
| LZ-PROJECT-INVALID | Path is absent, unsafe, or not Git-backed | Return exact validation reason |
| LZ-PROJECT-CONFLICT | Registration conflicts with existing identity | Return existing project reference |
| LZ-DAEMON-UNAVAILABLE | Keeper exhausted restart budget | Spool and notify outside the daemon |

Every error crosses a boundary as a typed value. Panics are process defects, not ordinary control flow. The keeper treats unexpected daemon termination as evidence and never hides it through infinite restarts.

## 14. Observability and resource accounting

Structured local logs include timestamp, level, component, boot identifier, request identifier, project identifier when relevant, event sequence, duration, and stable error code. They exclude secrets and full command payloads by default.

system.status reports:

- daemon and CLI versions;
- boot identifier and uptime;
- protocol version;
- data-root path;
- database schema version and WAL status;
- project count;
- audit high-water sequence;
- pending outbox count;
- last emergency-spool reconciliation;
- keeper restart-budget observation read from the atomic keeper state file; and
- degraded conditions.

The slice captures idle CPU, memory, handles, threads, database latency, pipe round-trip latency, and log/spool growth on the owner's baseline laptop. It records evidence but does not yet implement the full 20 percent resource governor.

## 15. Testing strategy

### 15.1 Unit tests

- checkout-mode parsing and defaults;
- project path and identity validation through injected filesystem/Git ports;
- idempotent project registration;
- event and configuration revision allocation;
- keeper restart-window and backoff calculations;
- spool encoding, deduplication, rotation, and partial-line recovery;
- protocol serialization and strict-field validation; and
- stable error-to-exit-code mapping.

### 15.2 Store integration tests

- clean migration and reopen;
- WAL and foreign-key configuration;
- atomic project, audit, and outbox commit;
- duplicate idempotency key;
- rollback on injected failure;
- pending outbox replay;
- monotonic sequence after restart;
- unknown or checksum-changed migration;
- emergency-spool import exactly once; and
- isolated temporary data roots.

### 15.3 Windows IPC tests

- current-user connection succeeds;
- invalid nonce fails before domain access;
- incompatible protocol fails cleanly;
- remote-client rejection flag and explicit DACL are present;
- bounded oversized frame is rejected;
- a subscription receives persisted catch-up and then live events;
- client reconnects and resumes after its last acknowledged sequence;
- concurrent status reads do not starve a mutation; and
- pipe teardown produces daemon-unavailable rather than a hung CLI.

An additional baseline-machine security gate inspects the effective pipe ACL. Cross-user denial is tested where the test environment can create a second local token; otherwise it remains a clearly reported manual gate rather than an inferred pass.

### 15.4 Keeper process tests

- healthy daemon start;
- unexpected exit followed by restart;
- persisted restart budget across keeper restart;
- restart-budget exhaustion;
- emergency-spool durability while SQLite is locked or unavailable;
- notifier invocation after spool success;
- notifier failure leaves spool success intact; and
- recovered daemon imports emergency incidents exactly once.

### 15.5 End-to-end acceptance

The acceptance script must:

1. use a fresh isolated Lazoo data root and temporary Git repository;
2. start the keeper and wait for daemon readiness without arbitrary sleeps;
3. run lazooctl status successfully;
4. register the temporary project with the default isolated checkout mode;
5. verify project.list and events.list;
6. terminate the daemon unexpectedly;
7. observe keeper restart within the bounded policy;
8. verify the project, event sequence, and outbox state survived;
9. force restart-budget exhaustion with a deliberately crashing test daemon while keeping the production restart constants unchanged;
10. verify the emergency incident spool before daemon recovery;
11. restart cleanly and verify exactly-once spool reconciliation; and
12. emit a machine-readable PASS/FAIL manifest with command output, versions, timings, and artifact paths.

No gate is reported as passed unless its command actually ran. Unsupported cross-user or interactive-notification assertions are listed as explicit manual or blocked gates.

## 16. Acceptance criteria

The slice is complete only when:

- all three executables build from a clean checkout on the baseline Windows machine;
- unit, store, Windows IPC, keeper, and end-to-end suites pass;
- project registration remains durable across a forced daemon restart;
- an idempotent repeated registration produces one logical mutation;
- the control pipe rejects invalid nonce proof and has the intended current-user ACL;
- restart-budget exhaustion stops the loop, durably spools LZ-DAEMON-UNAVAILABLE, and attempts native notification;
- recovery imports the emergency incident exactly once;
- the CLI returns stable human and JSON output;
- formatting, lint, dependency, license, and security checks defined by the implementation plan pass;
- measured idle and active resource evidence is captured; and
- the implementation does not add provider, terminal, desktop, memory, or worktree-creation scope.

## 17. Implementation-order constraints

The implementation plan should preserve these dependencies:

1. protocol and domain types before transports;
2. migration and transactional repositories before daemon commands;
3. named-pipe security and initialization before domain methods;
4. daemon lifecycle before keeper supervision;
5. emergency spool before restart-budget exhaustion behavior;
6. CLI commands only after the matching daemon method exists; and
7. end-to-end acceptance after each preceding boundary has focused tests.

Tests should be written before or alongside behavior, not added as a final cleanup phase.

## 18. Deferred decisions

The implementation plan will select exact current crate versions and document why. Later slices decide:

- Tauri UI framework and desktop lifecycle;
- provider adapter runtime details;
- terminal streaming contract;
- generalized Job Object governance;
- worktree creation and integration;
- employee and task schemas;
- policy and approval persistence;
- managed-memory workers;
- installer identity, auto-start, and update channel; and
- configurable keeper thresholds.

None of these deferred decisions may weaken the security, durability, recovery, or audit contracts established here.

## 19. Review questions resolved by owner direction

The design uses the owner-approved decisions:

- greenfield vertical slices;
- keeper and dual-path failsafe in the first runnable milestone;
- Ask as the default authority mode for later policy slices;
- all editing employees isolated by default, with an optional lead-main project mode;
- managed indexed memory as the later default, with worker/API choice;
- source transcripts retained by default with global and project opt-out; and
- Claude live harness testing through an external endpoint/model, with the Anthropic subscription path tracked separately.

Those later-product choices are recorded here only to prevent this foundation from closing off their required seams.
