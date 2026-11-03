# Account routing preferences implementation plan

Execute inline in the existing worktree, following the owner's approved specification.

- [x] Add protocol contracts and migration 0022; propagate schema assertions.
- [x] Implement revision-fenced reserves, priority ordering, exhaustion and current-window admission in the store.
- [x] Add transactional hourly aggregation with cumulative token watermarks and bounded read queries.
- [x] Normalize Codex, Claude and API header observations; retain exact session attribution.
- [x] Expose owner RPCs, CLI commands, desktop allowlist and TypeScript types; add account labels to session read models.
- [x] Add focused routing, ledger, normalization, redaction and RPC regression tests.
- [x] Run formatting, workspace clippy and workspace tests using CARGO_BUILD_JOBS=2.
- [x] Document exact RPCs, observability and verification limits; save PR-BODY.md for the owner to publish against main.
- [ ] Commit locally (final staging retry also denied access to the worktree index.lock outside the writable root).
