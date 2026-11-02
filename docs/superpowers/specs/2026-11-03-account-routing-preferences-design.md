# Account routing preferences and usage

This slice extends PR #42's daemon-owned account routing. Identity and credential handling stay unchanged. The desktop receives public metadata only; no component work is included.

## Routing

`priority` chooses the first eligible account in `order`. Unlisted accounts follow in creation order (UUID breaks timestamp ties). `balanced` sorts by selection count, then the same order. `api-pool` uses priority when `order` is nonempty, otherwise its existing least-selected behavior. Order entries must be distinct, live accounts of the policy's provider.

`provider.accounts.preferences` changes only `reservePercent`, with the existing account revision fence. Values are null or 0–90. A new session is ineligible when any current, observed quota window reaches `100 - reservePercent`. Windows without a valid future reset cannot enforce a reserve. Missing observations remain explicitly unenforceable, rather than manufacturing quota knowledge. An observed 100% window blocks new routing even without a reserve.

Structured quota rejection sets `exhaustedUntil` from a reported reset, bounded to 30 days, or a 15-minute default. API HTTP 429 shares that state and the existing cooldown. Disabled, deleted, cooling, exhausted, restricted, or reserved accounts never cause fallback to an ambient login. Existing bindings and native resumes retain their account; preference changes never move them. Reserve admission applies only to new sessions. Existing cooldown behavior on resumes remains fail-closed.

## Observation

Codex 0.153.4's locally generated app-server schema confirms `account/rateLimits/read`, `account/rateLimits/updated`, `primary`/`secondary`, `usedPercent`, `windowDurationMins`, epoch-second `resetsAt`, `planType`, and `codexErrorInfo: usageLimitExceeded`. Reads use the existing session process without submitting a model prompt. Notifications and structured failures use the same normalization path.

Claude Code 2.1.316 has documented statusline `rate_limits` and stream `rate_limit_event`. Lazoo captures `rate_limits` through a session-only statusline in its own native Claude sessions and handles exact `StopFailure.error: rate_limit`. Personal settings remain unchanged. The legacy stream adapter also captures `rate_limit_event`. Statusline observations are retained in a bounded slot and drained on ordinary hooks, account reads, or new routing. No scraping, private endpoints, token extraction, or model usage probes are allowed. API windows come exclusively from upstream response headers. ACP context utilization is not subscription quota. Unsupported subscription windows are reported as unavailable.

Only allowlisted numbers, enums, timestamps, and locally assigned account IDs enter usage windows. Provider account identifiers, arbitrary headers, error text, and authentication values are excluded.

## Ledger and contracts

Migration 0022 adds latest quota windows, hourly usage counters, and token watermarks. Writes join existing session-ready, turn-event, turn-failure, and proxy-metric transactions. Cumulative provider usage is differenced, never summed repeatedly. Null input/output token totals mean no report for that direction; `tokensReported` makes partial coverage explicit. Sessions count successful starts (including resume activations); failures count failed turn attempts, not each diagnostic error.

Retention is 90 days. The owner-only usage RPC defaults to seven days and UTC day buckets; ranges are at most 90 days and result sets at most 10,000 rows. Omitted `accountId` means all accounts, explicit null means unmanaged login. Results include latest windows independently of the analytics interval and provider capability/reason metadata. Session, lead, and worker read models expose a nullable `{ id, label }` account summary.

## Validation

Focused coverage checks order, creation fallback, reserve thresholds and missing/reset observations, exhaustion and expiry, binding preservation, fail-closed routing, aggregation and token deduplication, filters and limits, RPC authorization/shapes, and secret exclusion. Update every schema contract to 22. Run fmt, workspace clippy and workspace tests with two jobs and the existing target. Release build and full gate remain the owner's pre-merge checks.
