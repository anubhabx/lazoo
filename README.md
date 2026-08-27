# Lazoo

Lazoo is a Windows-first, local desktop control plane for a personal team of native coding-agent harnesses. It is intended to coordinate Codex, Claude Code, OpenCode, Grok Build, and Antigravity employees without replacing or flattening their native environments.

The name is a deliberately silly blend of **lazy** and **zoo**: wake the right employee when work arrives, let the team coordinate, and keep the menagerie under control.

## Status

Lazoo is currently in research and architecture definition. There is no product implementation yet.

The researched product and engineering contract lives in [docs/PROJECT-BRIEF.md](docs/PROJECT-BRIEF.md).

## Operating boundary

- Windows first
- One local user
- Native provider sessions and terminals
- Git worktrees by default for editing assignments
- Configurable authority, with `ask` as the default
- Local control plane with no network listener by default

Implementation should not begin until the project brief has been reviewed and a separate implementation plan has been approved.
