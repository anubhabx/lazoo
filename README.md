# Lazoo

Lazoo is a Windows-first, local desktop control plane for a personal team of native coding-agent harnesses. It is intended to coordinate Codex, Claude Code, OpenCode, Grok Build, and Antigravity employees without replacing or flattening their native environments.

The name is a deliberately silly blend of **lazy** and **zoo**: wake the right employee when work arrives, let the team coordinate, and keep the menagerie under control.

## Status

The first Windows control-plane slice is implemented. It provides three native executables:

- `lazoo-keeper.exe` supervises the daemon with a bounded restart policy and emergency spool.
- `lazood.exe` owns the SQLite WAL store and authenticated named-pipe API.
- `lazooctl.exe` exposes status, project registration/listing, event listing/following, and planned shutdown.

Provider adapters, employee execution, the desktop shell, managed memory, and worktree automation remain later slices. The full product contract lives in [docs/PROJECT-BRIEF.md](docs/PROJECT-BRIEF.md).

## Build and run

Use the pinned Rust toolchain on Windows:

```powershell
& "$env:USERPROFILE\.cargo\bin\cargo.exe" build --workspace --release
& .\target\release\lazoo-keeper.exe
```

In another terminal:

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
& "$env:USERPROFILE\.cargo\bin\cargo.exe" test --workspace
& "$env:USERPROFILE\.cargo\bin\cargo.exe" check --workspace --all-targets
& "$env:USERPROFILE\.cargo\bin\cargo.exe" clippy --workspace --all-targets -- -D warnings
& "$env:USERPROFILE\.cargo\bin\cargo.exe" fmt --all -- --check
powershell -NoProfile -ExecutionPolicy Bypass -File scripts\acceptance\first-slice.ps1
```

The acceptance script uses a unique temporary data root, builds release binaries, forces a real daemon restart, exhausts the production keeper budget with a crashing fixture, and writes `artifacts\acceptance\first-slice-manifest.json`.

## Operating boundary

- Windows first
- One local user
- Native provider sessions and terminals
- Git worktrees by default for editing assignments
- Configurable authority, with `ask` as the default
- Local control plane with no network listener by default

Each later slice still requires a reviewed design, a written implementation plan, and acceptance evidence.
