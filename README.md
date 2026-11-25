<!-- banner -->
# Lazoo

A local Windows desktop for running a small team of coding agents, each in its own native terminal UI.

The name is a blend of **lazy** and **zoo**: wake the right agent when work arrives, let the team coordinate, and keep the menagerie under control.

## What it is

- One lead agent plans work and delegates tasks to worker agents, and the workers can run on different harnesses (Codex, Claude Code, GitHub Copilot, OpenCode, Antigravity).
- Every agent runs in its own genuine native TUI. Lazoo does not replace or flatten each harness's own configuration, login or terminal.
- Approval requests and anything else that needs you land in an Inbox. Authority is set per task, and `ask` is the default.
- Several accounts per provider are supported, with isolated native profiles and per-provider routing policies. See [docs/ACCOUNT-ROUTING.md](docs/ACCOUNT-ROUTING.md).
- Skills and plugins can be installed once and shared with the harnesses you pick (Settings, then Skills and Plugins).
- Voice mode lets you dictate into the conversation, hear replies read aloud, or use a global push-to-talk hotkey. Speech to text runs on your computer by default.
- Everything is local: a per-user daemon, a SQLite store and a named-pipe API. There is no Lazoo cloud service and no network listener by default.

## Status

Lazoo is pre-1.0 and Windows 10/11 only. Adapters exist for Codex, Claude Code, GitHub Copilot, OpenCode and Antigravity; Grok Build is excluded from v1. Verification depth differs by harness and by feature, so expect rough edges.

The slice-by-slice record of what is built and verified is in [docs/STATUS.md](docs/STATUS.md). The product design is in [docs/PROJECT-BRIEF.md](docs/PROJECT-BRIEF.md).

## Install

There is no published release binary yet, so build the installer yourself:

```powershell
pwsh -File scripts\build-installer.ps1
```

It builds a per-user Windows installer that needs no administrator rights. The installer places `lazoo-desktop.exe` (Start menu: Lazoo) and its service binaries in `%LOCALAPPDATA%\Programs\Lazoo`. Data stays in `%LOCALAPPDATA%\Lazoo`. Opening the app starts its local service.

## Build from source

You need Windows, the Rust toolchain pinned in `rust-toolchain.toml` (rustup installs it on first use), and Node 24.

```powershell
npm --prefix apps\lazoo-desktop\ui ci
npm --prefix apps\lazoo-desktop\ui run build
cargo build --workspace --release --locked
& .\target\release\lazoo-keeper.exe
```

Open the desktop from another terminal:

```powershell
& .\target\release\lazoo-desktop.exe
```

Closing the window leaves the service and the agents running. `lazooctl system shutdown` stops them. Set `LAZOO_DATA_ROOT` to point a test run at an isolated data folder.

For the frontend loop, run `cargo tauri dev` from `apps\lazoo-desktop` with Tauri CLI 2 installed. It starts Vite on `127.0.0.1:1420`.

## Architecture

| Executable | Role |
| --- | --- |
| `lazoo-desktop` | The Tauri 2 desktop shell. Shows conversations, the team, the Inbox and settings, and hosts each agent's native TUI. |
| `lazoo-keeper` | Supervises the daemon with a bounded restart policy and an emergency spool. |
| `lazood` | The daemon. Owns the SQLite store, the roster, native sessions, messages, conversations, tasks and the named-pipe API. |
| `lazooctl` | The command-line client for the daemon. |
| `lazoo-session-host` | Launch gate that places each native provider process tree in a daemon-owned Windows Job Object before the provider starts. |

| Crate | Role |
| --- | --- |
| `crates/lazoo-domain` | Core types and rules for employees, tasks, messages, conversations, incidents and the Inbox. |
| `crates/lazoo-ipc` | Authenticated named-pipe connections and framing. |
| `crates/lazoo-protocol` | The RPC request and result types shared by the daemon, CLI and desktop. |
| `crates/lazoo-providers` | Adapters for each harness, plus configuration discovery and the skills and plugins layer. |
| `crates/lazoo-runtime` | Windows runtime pieces: data root, Job Objects, terminals, single-instance locks, worktrees. |
| `crates/lazoo-store` | SQLite persistence and schema migrations. |

## CLI

`lazooctl` talks to the running daemon. Add `--json` for machine-readable output.

```powershell
lazooctl status
lazooctl project register C:\path\to\git-repository
lazooctl provider list
lazooctl employee create Rhea --provider codex --role Lead
lazooctl turn submit <conversation-id> --text "Describe the repository." --activate-if-needed
lazooctl task list
lazooctl approval list --state pending
lazooctl inbox list --filter all
```

The full command reference, with every option and exit code, is in [docs/STATUS.md](docs/STATUS.md) under "Build and run".

## Operating boundary

These limits come from the project brief and still hold:

- Windows first, one local user.
- Native provider sessions and terminals, not re-implemented ones.
- Git worktrees by default for assignments that edit files.
- Configurable authority per task: `ask`, `self-decision` or `full-autonomy`, with `ask` as the default.
- A local control plane with no network listener by default.
- The desktop only sends actions the daemon offers it, through a fixed allowlist. It never names an executable.

## Repository layout

- `apps/` holds the five executables, and the desktop UI under `apps/lazoo-desktop/ui`.
- `crates/` holds the shared Rust libraries listed above.
- `docs/` holds the project brief, the status record, design specs, plans and verification notes.
- `scripts/` holds the installer build, the local gate and the acceptance scripts.
- `design-system/` holds the design tokens used by the desktop UI.

## Contributing

- Name branches `feat/`, `fix/`, `chore/`, `docs/` or `test/` followed by a short description.
- Write conventional commit subjects, for example `fix: stop a stale session from blocking restart`.
- Run `pwsh -File scripts/gate.ps1` before opening a PR. It runs the full local gate: format, clippy, tests, the release build and the acceptance scripts. CI is only a cheap backstop (format, UI check and clippy).
- Run heavy builds one at a time. Parallel cargo builds can exhaust memory and disk on a small machine.
- The design system lives in `design-system/`. UI changes should use its tokens.
- Fill in the pull request template, and update the docs when behavior changes.

## License

MIT. See [LICENSE](LICENSE).
