# Lazoo

Lazoo is a Windows-first, local desktop control plane for a personal team of native coding-agent harnesses. It is intended to coordinate Codex, Claude Code, OpenCode, Grok Build, and Antigravity employees without replacing or flattening their native environments.

The name is a deliberately silly blend of **lazy** and **zoo**: wake the right employee when work arrives, let the team coordinate, and keep the menagerie under control.

## Status

The first Windows control-plane slice and the read-only desktop-shell slice are implemented. Lazoo now provides four native executables:

- `lazoo-desktop.exe` opens a Tauri 2 control room for daemon health, projects, recent events, and recovered-incident count.
- `lazoo-keeper.exe` supervises the daemon with a bounded restart policy and emergency spool.
- `lazood.exe` owns the SQLite WAL store and authenticated named-pipe API.
- `lazooctl.exe` exposes status, project registration/listing, event listing/following, and planned shutdown.

The desktop is deliberately read-only in this slice. Provider adapters, employee execution, task delegation, managed memory, and worktree automation remain later slices. The full product contract lives in [docs/PROJECT-BRIEF.md](docs/PROJECT-BRIEF.md).

## Build and run

Use the pinned Rust toolchain on Windows:

```powershell
cargo build --workspace --release --locked
& .\target\release\lazoo-keeper.exe
```

Open the desktop from another terminal:

```powershell
& .\target\release\lazoo-desktop.exe
```

The shell reconnects every ten seconds and keeps the last successful snapshot visible if the daemon becomes unavailable. It does not start or stop the keeper. The companion CLI remains available for control operations:

```powershell
& .\target\release\lazooctl.exe status
& .\target\release\lazooctl.exe project register C:\path\to\git-repository
& .\target\release\lazooctl.exe project list
& .\target\release\lazooctl.exe events list
& .\target\release\lazooctl.exe system shutdown
```

Add `--json` for a stable machine-readable result. Mutable state defaults to `%LOCALAPPDATA%\Lazoo`; set `LAZOO_DATA_ROOT` to isolate a test installation.

## Verification

```powershell
& .\scripts\verify-rust-toolchain.ps1
cargo test --workspace
cargo check --workspace --all-targets
cargo clippy --workspace --all-targets -- -D warnings
cargo fmt --all -- --check
powershell -NoProfile -ExecutionPolicy Bypass -File scripts\acceptance\first-slice.ps1
node --test apps\lazoo-desktop\ui\view-model.test.mjs
powershell -NoProfile -ExecutionPolicy Bypass -File scripts\acceptance\desktop-shell.ps1
```

The acceptance script uses a unique temporary data root, builds release binaries, forces a real daemon restart, exhausts the production keeper budget with a crashing fixture, and writes `artifacts\acceptance\first-slice-manifest.json`.

The desktop-shell acceptance starts an isolated keeper and daemon, registers a disposable Git repository, opens the real WebView2 window, verifies connected content through Windows UI Automation, checks default and minimum-size captures, and writes `artifacts\acceptance\desktop-shell-manifest.json`.

## Operating boundary

- Windows first
- One local user
- Native provider sessions and terminals
- Git worktrees by default for editing assignments
- Configurable authority, with `ask` as the default
- Local control plane with no network listener by default
- Read-only desktop snapshot; control mutations remain in the CLI until separately designed

Each later slice still requires a reviewed design, a written implementation plan, and acceptance evidence.
