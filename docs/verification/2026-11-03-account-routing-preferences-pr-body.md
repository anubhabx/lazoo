New sessions can drain accounts in an explicit order, preserve observed quota reserves, and move past structured quota exhaustion. The owner can query per-account hourly/day usage and see the bound account label in session, lead, and worker read models.

## Routing and storage

- Add priority mode and policy order. Balanced uses order for selection-count ties; api-pool uses priority when order is nonempty and retains least-selected selection when it is empty. Unlisted accounts follow by creation time.
- Add revision-fenced reservePercent (null or 0–90). Only current provider percentage windows with a future reset can enforce reserves; absent data is explicitly unenforceable. Structured exhaustion excludes new sessions until a bounded reset/default.
- Preserve account identity, accepted/resumed ownership, and fail-closed LZ-ACCOUNT behavior. No automatic switch of accepted sessions, ambient fallback, credential extraction, private subscription endpoints, or model usage probes.
- Migration 0022 adds latest public windows, hourly counters and cumulative token watermarks. Counters share existing write transactions, retain 90 days, and preserve unknown versus reported-zero tokens. All store/daemon/acceptance schema contracts move to 22.
- Add lazooctl account usage, account preferences, and account policy <provider> priority --order <id,...>.
- Desktop changes are restricted to the Rust allowlist/count assertions and protocol.ts types. Read allowlist: 44; mutation allowlist: 31. No component changes.

## Exact owner RPC shapes

All methods below are owner control-plane RPCs; they are not worker MCP tools.
Desktop reads use `Kind::Read`, preferences and policy use `Kind::Mutation`.
Optional desktop mutation `idempotencyKey` is accepted; durable revisions fence writes.

```typescript
// provider.accounts.policy (revision is the expected policy revision)
{ provider: string, mode: "inherited" | "balanced" | "priority" | "pinned" | "api-pool",
  accountId?: string | null, revision: number, order?: string[] }
// -> ProviderAccountPolicy { provider, mode, accountId, revision, order }

// provider.accounts.preferences
{ id: string, expectedRevision: number, reservePercent: number | null }
// -> ProviderAccountRecord

// provider.accounts.list {}
// -> { version: 1, accounts: ProviderAccountRecord[], policies: ProviderAccountPolicy[] }
// Record additions:
{ createdAt: string, reservePercent: number | null, exhaustedUntil: string | null,
  reserveEnforceable: boolean, reserveReason: string | null, windows: UsageWindow[] }

// provider.accounts.usage
{ from?: string, to?: string, bucket?: "hour" | "day", provider?: string,
  accountId?: string | null }
// ->
{ buckets: Array<{ start: string, provider: string, accountId: string | null,
    sessions: number, turnsCompleted: number, turnsFailed: number,
    requests: number, failures: number, inputTokens: number | null,
    outputTokens: number | null, tokensReported: number }>,
  windows: UsageWindow[],
  observableProviders: Record<string, { windows: boolean, tokens: boolean, reason: string | null }> }

type UsageWindow = {
  provider: string; accountId: string | null; kind: string;
  usedPercent: number | null; remaining: number | null; limit: number | null;
  windowMinutes: number | null; resetsAt: string | null; observedAt: string;
  source: string; planType: string | null;
};

// EmployeeSessionRecord, LeadSessionSummary and WorkerSessionSummary add:
{ account: { id: string, label: string } | null }
```

Timestamps are RFC3339. `accountId` omitted queries all accounts; explicit null selects
unmanaged personal login. Defaults are the last seven days and UTC day buckets. The
range must be within the last 90 days, at most 90 days long. Hourly storage means
partial-hour endpoints include their intersecting hour. Results exceeding 10,000
buckets or windows fail with an input error instead of silently truncating. Windows
are the latest observed values independently of the selected analytics interval.
No historical usage is backfilled from old transcripts.

Sessions count successful start/resume activations. Completed/failed turns count
durable outcomes (failed submission attempts included), not repeated error messages.
The API owner before acceptance can change only through the existing explicit
rejection flow; the start count moves with that binding CAS. Proxy attempts stay
attributed to the credential actually attempted. Token counters only include provider
reports. `null` means no report for that direction; reported zero stays zero, and
`tokensReported` counts observed directional reports. Codex thread totals and other
per-turn snapshots use durable high-water marks to avoid summing cumulative usage.

Migration 0022 retains 90 days of hourly counters and token watermarks. Aggregation
runs inside existing session/turn/proxy transactions, with no additional ledger
fsync. Latest quota observations contain only allowlisted kinds/sources, numeric
values, timestamps and optional public plan enums. They never include upstream
account IDs, raw headers, credentials, error text or screen contents.

Evidence: installed Codex `0.153.4` schema generation; Claude Code `2.1.316` binary
surface check plus [statusline reference](https://code.claude.com/docs/en/statusline),
[StopFailure hook reference](https://code.claude.com/docs/en/hooks#stopfailure), and
[structured stream reference](https://code.claude.com/docs/en/agent-sdk/typescript#sdkratelimitevent).
Header formats follow [OpenAI](https://developers.openai.com/api/docs/guides/rate-limits)
and [Anthropic](https://platform.claude.com/docs/en/api/rate-limits).
See also [Copilot ACP](https://docs.github.com/en/copilot/reference/copilot-cli-reference/acp-server)
and [OpenCode token schema](https://github.com/anomalyco/opencode/blob/dev/packages/schema/src/token-usage.ts).

## TypeScript handoff

The exported types are AccountRoutingMode, ProviderAccountPolicy, ProviderAccountRecord, UsageWindow, AccountUsageParams, AccountPreferencesParams, AccountUsageBucket, AccountUsageResult, SessionAccount, and WorkerSessionSummary. Existing EmployeeSessionRecord and LeadSessionSummary add account: SessionAccount | null; WorkerCard.session uses WorkerSessionSummary | null. Policy responses always include order; requests may omit it (empty default). ProviderAccountSpec is unchanged.

Relative to the initial protocol.ts diff, there are no subsequent wire-shape changes. The final additions include createdAt, tokensReported and UsageWindow.planType. Capability booleans describe supported adapter paths, not guaranteed data for a login; UI should display reason and actual nullable observations.

## Provider observability

| Provider / transport | Quota windows | Token ledger | Exhaustion evidence |
| --- | --- | --- | --- |
| Codex app-server | `primary`, `secondary`; percent, minutes, reset, optional plan | Reported thread input/output totals, differenced; first report uses `last` to exclude prior history where supplied | `codexErrorInfo` on error or failed turn |
| Claude native TUI | Supported statusline `rate_limits.five_hour` / `seven_day` in Lazoo-owned sessions | Not reported by these native hooks; context occupancy is excluded | `StopFailure.error == "rate_limit"` |
| Claude stream adapter | Structured `rate_limit_event`, when emitted | `result.usage` input/output, when emitted | Typed `rejected` state, accounting for allowed overage |
| API pool (Codex, Claude, OpenCode) | OpenAI `x-ratelimit-*` or Anthropic `anthropic-ratelimit-*` response headers, per serving credential | Provider events above; proxy counts requests/failures, not response-body token estimates | HTTP 429 and retry/reset headers |
| OpenCode owned HTTP | No native subscription windows; API pool headers when configured | Owned assistant `message.updated` input/output counts, summed across messages and differenced within a turn | No verified native subscription exhaustion code |
| GitHub Copilot ACP | No verified structured subscription windows | Directional `turn_completed.usage` only when supplied; context `usage_update.used` is excluded | No verified quota-specific ACP error schema; generic errors never imply exhaustion |
| Antigravity | No verified subscription windows | Legacy result token fields if emitted; governed native hooks do not report them | Not observable |
| Grok Build | Not observable | Not observable | Existing adapter limitations apply |

`observableProviders` describes adapter capability, not a promise that a particular
account has supplied data. Read the reason and actual nullable totals/windows.
For Claude/Antigravity, `tokens: true` refers only to the legacy machine adapter;
the governed native path remains unreported. OpenCode's input/output fields retain
their provider definitions; separate cache/reasoning counts are not invented or
folded into those directions.


Copilot's separate installed SDK declares account.getQuota, but that is not the integrated ACP transport and has not been connected or account-attribution verified. Native OpenCode and Antigravity subscription exhaustion has no verified structured signal in their integrated interfaces. These remain explicitly unobservable.

## Validation

- PASS: cargo fmt --all; changed Rust sources also checked for CRLF.
- PASS: cargo clippy --workspace --all-targets -- -D warnings.
- PASS: focused routing, ledger, quota/header normalization, native callback, CLI, RPC and allowlist tests. Command: cargo test --workspace --lib --test provider_accounts --test codex --test opencode --test cli_deadlines_contract --no-fail-fast. Its only remaining failure was the old Codex event-position expectation; after updating that assertion to include the new quota event, cargo test --workspace --test codex passed (21 passed, 3 existing live tests ignored).
- PASS: final cargo test --workspace, exit 0. All 153 daemon tests passed, including the acceptance-script literal contracts and the task-graph execution. Existing live-provider tests remain ignored.
- PASS: task-graph acceptance with current debug fixtures and SkipBuild (schema 22, all evidence checks true). The workspace test now uses this same path.
- PASS: git diff --check and UI scope check (only protocol.ts changed under the UI tree).

All Cargo runs used CARGO_BUILD_JOBS=2 and the existing target. Final logs are artifacts/account-preferences-clippy-final.log, account-preferences-codex-final.log and account-preferences-workspace-complete.log. Intermediate failures remain in account-preferences-focused-tests.log, account-preferences-workspace-tests.log and account-preferences-workspace-final.log. An intermediate nested debug build exhausted disk; the owner freed space by deleting incremental caches, and the final suite passed with approximately 10 GB still free. The test harness no longer performs a redundant nested workspace build.

Installed Codex 0.153.4 app-server schema generation verifies exact quota methods/fields and typed errors; compact schema evidence and hashes are in docs/verification/2026-11-03-account-routing-protocol-evidence.json. Installed Claude Code 2.1.316 and official statusline/hooks/stream documentation confirm the structured surfaces. No live model requests were spent to test limits.

The full scripts/gate.ps1 is left to the owner. The final workspace-test harness uses debug builds. An initial test run unexpectedly invoked the existing task-graph script's release build; it completed before discovery. This accidental run is recorded and is not presented as a full release gate. Standalone acceptance keeps its release default. Live authenticated quota capture, real exhaustion, and cross-account live resume were not exercised. Desktop component integration/UI build is left to the concurrent UI work. Push and PR creation are delegated to the owner per the final handoff instruction.

The provider cancellation test helpers now use Get-Process instead of tasklist: tasklist returns Access denied in this sandbox even for the caller's PID and incorrectly treated that denial as a missing process. The revised helpers assert successful enumeration and retain the real child-liveness checks. OpenCode ACP fixtures use isolated profile environments instead of depending on personal configuration. The task-graph test reuses existing debug fixtures with SkipBuild and checks that its desktop diff matches the entry baseline, allowing the authorized protocol/allowlist edits while still detecting mutations.
