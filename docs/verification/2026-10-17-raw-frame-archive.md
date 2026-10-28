# Raw provider-frame retention verification

Date completed: 2026-10-17 IST

Branch: `worktree-wf_0f2eb737-354-1` (worktree `C:\workspace\Orchestrator\.claude\worktrees\wf_0f2eb737-354-1`)

Base: `f98ab93` (core v1 backend checkpoint)

Product version: `0.1.0`

Database schema: 19 (migration `0019_memory_raw_frames.sql`)

Scope: `source_kind` on `memory_archives` and `memory_archive_intents`, `Store::archive_memory_raw_frames` and `Store::read_memory_raw_frames`, the shared archive write path, kind-safe candidates/reconcile/cleanup/record checks, the daemon's per-turn raw-frame capture in `AppTurnObserver` and its archive call at turn end, the schema bump from 18 to 19 in every assertion and acceptance manifest, the `raw-frame-archive.ps1` acceptance script with its contract test, and this record. The desktop app, Tauri surface, `desktop.snapshot`, provider adapters, CLI and RPC surface are unchanged.

## Outcome

PASS within the limits named below.

Section 27 says compressed source transcripts are retained by default within visible limits. Until now only the bounded canonical turn was retained: `bounded_event_payload` removed every `ProviderEvent` `raw` payload before it reached the transcript, and nothing kept it. The daemon now buffers the adapter-redacted `raw` payload of each mapped provider event while the turn is live, bounded by the inherited policy's `max_source_bytes`, and when the turn ends `completed` it writes one `raw-frames` archive for the turn through the same hash, zstd, write-pending intent, quota and integrity path as the canonical archive. The archive body is NDJSON: a header line `{type:"lazoo-raw-frames",version:1,sourceTurnId}`, one line per retained frame `{sequence,type,raw}` after the store's own redaction, and, when any frame was dropped at the cap, a trailer `{type:"lazoo-raw-frames-truncated",droppedFrames,limitBytes}` with the archive row's `truncated` set. The canonical transcript still carries no `raw`.

No live model turn was run.

## Design notes

| Decision | Reason |
|---|---|
| `source_kind` with `PRIMARY KEY (source_turn_id, source_kind)` on both archive tables, rebuilt in migration 19 | Both tables keyed archives by turn alone; a second kind for the same turn needs the key widened. Existing rows become `turn`; `UNIQUE(scope_key, source_hash)`, `UNIQUE(archive_name)` and the intents name index are kept. |
| Capture in the daemon per mapped `ProviderEvent`, not at the wire readers | Adapters already redact `raw`; the one observer is already built once per turn; no adapter changes. Frames that map to no event are therefore not retained (see boundaries). |
| Completed turns only, decided in the daemon and enforced again in the store | Parity with canonical candidates (`t.state='completed'`); a retried turn reuses its id, and a failed attempt leaves nothing to overwrite. |
| Same retention opt-out, same `max_source_bytes`, same per-scope quota | One policy the user already controls; raw frames cannot grow the archive beyond the quota the user set. The store re-reads the policy at archive time, so disabling retention mid-turn drops the capture. |
| Consecutive duplicate raw payloads kept once | Claude's adapter clones one native message into several events; the duplicate would only fill the cap faster. |
| Header line keyed to the turn | Two turns with identical frames would collide on `UNIQUE(scope_key, source_hash)`; the header makes each turn's body distinct. |
| Trailer space reserved before the first frame | Appending the trailer can never breach the cap; a cap too small for header plus trailer is refused as `memory source`. |
| Sequence numbers count dropped frames | A gap in `sequence` shows where a frame was dropped without storing it. |
| Refusals are `tracing::warn` and never fail the turn | Parity with canonical quota rejections in the sweep. The raw kind writes no error intent and is not listed by `memory.queue.status`. |
| Raw kind never enqueued, never read by the worker | `enqueue_memory_summary` and `insert_record` resolve the `turn` kind only; the summary worker reads `read_memory_source`, which is `turn` only. |
| Reconcile keyed by `archive_name` | Replaying one kind's intent must not delete or promote the other kind's row for the same turn. |
| `memory.archived` carries `kind` | The audit trail names which archive was written; the payload shape is otherwise unchanged. |

## Fresh verification matrix

All commands were run from the worktree after the final change, with `CARGO_TARGET_DIR=C:\workspace\Orchestrator\target` as the build rules require.

| Command | Exit | Evidence |
|---|---:|---|
| `cargo fmt --all -- --check` | 0 | No formatting differences. |
| `cargo clippy -p lazood -p lazoo-store --all-targets --locked -- -D warnings` | 0 | Clean. |
| `cargo test -p lazoo-store --test memory --locked` | 0 | 14 passed, 0 failed; the three new tests `raw_frame_archive_is_hashed_capped_and_counted_against_quota`, `raw_frame_archive_respects_retention_opt_out`, `raw_frames_and_canonical_archive_coexist_and_cleanup_removes_both` and the eleven existing tests, with the positional `INSERT INTO memory_archives` fixture switched to named columns. |
| `cargo test -p lazoo-store --test store --locked` | 0 | 14 passed, 0 failed; schema 19 asserted in eight places; new `version_eighteen_archives_upgrade_as_canonical_turn_archives` opens a database at the schema-18 shape with positional archive and intent rows and proves both rows become `source_kind='turn'`, the intents name index exists, and there are no foreign-key violations. |
| `cargo test -p lazoo-store --test incident --locked` | 0 | 7 passed, 0 failed; schema 19 asserted. |
| `cargo test -p lazood --test daemon raw_ --locked` | 0 | 4 passed, 0 failed: `completed_turn_archives_redacted_raw_provider_frames`, `raw_frame_capture_is_skipped_when_retention_is_disabled`, `raw_frame_capture_is_bounded_by_the_policy_cap`, `raw_frame_archive_acceptance_contract_is_deterministic_and_backend_only`. |
| `cargo test -p lazood --test daemon acceptance_contract --locked` | 0 | 30 passed, 0 failed; every acceptance-contract test accepts its script's `schemaVersion = 19` literal. |
| `cargo test -p lazood --test daemon status_reports_protocol_store_and_boot_identity --locked -- --exact` | 0 | 1 passed; `system.status` reports `schemaVersion` 19. |
| `cargo test -p lazood --test daemon turn_submit_persists_before_provider_acceptance_and_acknowledges_on_evidence --locked -- --exact` | 0 | 1 passed; the canonical payload still has no `raw`. |
| `cargo test -p lazood --lib memory_worker --locked` | 0 | 7 passed, 0 failed. |
| `scripts\acceptance\raw-frame-archive.ps1` | 0 | PASS manifest, schema 19, `deterministic-fake-transport`, 9 commands (8 focused cargo tests by exact name and the desktop diff guard), every evidence flag true. |

Not run, by the build rules for this parallel slice: `cargo test --workspace`, the whole `lazood` daemon test binary, `cargo build --workspace --release --locked`, `scripts\gate.ps1`, the other acceptance scripts whose manifest literals were bumped to 19 (their `schemaVersion` asserts were changed by text substitution and are not re-proven here), `provider-adapters.ps1`, `desktop-shell.ps1`, and the desktop `npm` check. The integrator's full gate covers them.

## Raw-frame-archive acceptance evidence

`artifacts/acceptance/raw-frame-archive-manifest.json` records literal `PASS`, schema 19, and `deterministic-fake-transport`. Its focused commands prove:

- in the store, about 100 KiB of frames under a 64 KiB cap yield one `-raw-` archive whose decoded bytes hash to `source_hash`, whose first line is the header for that turn, whose last line is the truncation trailer with `droppedFrames > 0` and `limitBytes` 65536, whose frame lines have `token` redacted and never contain the secret, and whose exact byte count is the row's `uncompressed_bytes`; the same frames again return the recorded archive; setting the quota to the raw archive's compressed size makes the canonical `archive_memory_turn` fail with `memory quota`, with no error intent and no canonical row;
- with retention disabled the raw archive is refused as `memory retention` and the archive root holds nothing; nothing captured is `Ok(None)`; a turn that is not `completed` is refused as `memory source` even with retention on;
- a raw archive leaves the turn a canonical candidate and is refused by `enqueue_memory_summary`; the canonical archive then lands beside it with a different name, the candidate disappears, both kinds read back, cleanup preview lists both names and their summed bytes, apply removes both files and both rows, and reconcile drops a raw `write-pending` intent without a file while the re-created canonical archive stays readable and no error intent remains;
- a schema-18 database with positional archive and intent rows upgrades to 19 as `turn` rows with the intents name index intact and no foreign-key violations;
- through the daemon with the fake transport, a completed turn whose `turnStarted` carried `{private, token}` and whose one `textDelta` message was cloned into two events produces four NDJSON lines (header, `turnStarted` with `token` redacted and `private` kept, one `textDelta`, `turnCompleted` at sequence 3), one `-raw-` file, a `memory.archived` event with `kind: raw-frames` and `truncated: false`, the turn still listed as a canonical candidate, and canonical payloads without `raw`;
- with `memory.policy.set` retention off, the completed turn leaves no raw archive, an empty archive root, and no `memory.archived` event;
- with a 4096-byte cap, a 16 KiB `textDelta` raw payload is dropped at capture while the two small frames are kept, the trailer reports `droppedFrames` 1 and `limitBytes` 4096, the sequence gap shows the drop, the body is at most 4096 bytes, and the event records `truncated: true` with the body's `uncompressedBytes`; and
- schema is 19 and the desktop tree is unchanged.

## Boundary evidence

- `git diff --quiet` and `git diff --cached --quiet -- apps/lazoo-desktop` both passed in the acceptance run; no Tauri command, capability, `desktop.snapshot` field or frontend source changed.
- No adapter, `ndjson.rs` or `json_rpc.rs` change: capture is of the `raw` payload each adapter already attaches to a mapped `ProviderEvent`. Wire frames that map to no event (normalizers returning `Ok(None)`, JSON-RPC responses consumed by `JsonRpcProcess::request`, Claude hook frames) and session-level frames outside a turn (for example `sessionStarted` during `session.warm`) are not retained. Codex and ACP-based adapters attach only the frame's params as `raw`. A wire-level tap is out of scope for this slice and remains a named boundary.
- Only turns that end `completed` are retained; failed, cancelled and uncertain turns keep nothing. Internal summary-worker turns are never captured.
- The observer reads the policy once at turn start for the cap; the store reads it again at archive time for retention. Enabling retention mid-turn captures nothing for that turn; disabling it mid-turn drops the capture.
- A refused raw archive (retention, quota, cap, I/O) is a daemon `tracing::warn` line and never a turn failure or an error intent; `memory.queue.status` does not list it. Archive cleanup remains an explicit preview/apply; `retention_days` is still not enforced automatically, and raw frames raise disk use within the shared quota.
- No new CLI or RPC lists or shows archives; `memory.cleanup.preview` is still the only surface that names archive files, and it now names both kinds.
- The RAM bound per live turn is `max_source_bytes` of serialized frames (default 4 MiB, at most 64 MiB), held as `serde_json::Value` with its overhead; compression runs in `spawn_blocking` on the `turn.submit` response path after the provider future resolves.
- Build note for the integrator: with several worktrees on one `CARGO_TARGET_DIR`, cargo keys workspace members by their workspace-relative path, so a `lazoo-store` check artifact built from another worktree was once taken as fresh here and lacked this slice's export; touching the changed store sources made cargo rebuild from this worktree. The full gate from the merged main checkout rebuilds from newer files and does not hit this.
- `UI BACKEND CONTRACT: GO` remains intentionally withheld.

## Retained follow-up boundaries

A wire-level frame tap in the two shared readers with plumbing through every adapter session; retention for non-completed turns with per-attempt keys; a visible error intent or event for refused raw archives; a listing or show RPC for archive sources by kind; and automatic `retention_days` enforcement each remain separate owner decisions. Everything else NEXT-SESSION lists remains a separate reviewed slice.
