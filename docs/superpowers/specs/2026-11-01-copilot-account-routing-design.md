# Copilot and account routing

The owner requests GitHub Copilot support, management of multiple accounts/API keys,
and pooled routing, while a separate Claude worktree continues UI refresh. Implementation
is authorized; work remains on `codex/copilot-account-routing`, initially based on
`feat/desktop-ui-pilot` at `fc4ceed`. Recheck remote PRs and stack on the UI refresh if
it lands first. Never alter another worktree or the owner's existing provider login.

## Provider support

Add `github-copilot` (aliases `copilot`, `github-copilot`) using the installed CLI's
documented ACP mode. Reuse Lazoo's ACP permission observer, cancellation, resume,
resource ownership, and scoped MCP bridge. Discover the executable without invoking
a shell with interpolated arguments. Report only demonstrated capabilities. Native
takeover can resume a suspended session; a concurrent native view must not be claimed
unless an actual controller-isolated view is supported. Existing native providers
retain their actual TUIs. Probe initialization makes no model request and cannot
claim paid model/authentication readiness.

## Account pool

An account has an immutable UUID, provider, display name, credential mode, enabled
state, model restrictions, revision, and routing observations. Secrets are protected
for the current Windows user and excluded from RPC responses, audit events and Debug.
Native profile mode refers to an explicit absolute isolated provider configuration
directory; key mode supplies a per-child environment, never mutating global environment
or personal configuration. Configured pools fail closed when empty/unavailable rather
than silently using ambient credentials. No configured pool preserves existing behavior.

Native account selection is balanced for new sessions and pinned across resume; rotation
must never reinterpret another account's native session ID. Pinning is durable. Disabled
or deleted accounts cannot silently cause a different account to inherit a session.
API keys can use an authenticated loopback streaming proxy for compatible protocols.
Requests route among eligible credentials with cooldowns after authentication/quota
failures and sticky affinity for continuation. Never replay a partially delivered stream
or a request with ambiguous side effects. Transport failures have bounded timeouts.
Expose provider-specific routing capabilities, not fictitious universal compatibility.

## Product controls

Owner-only RPCs and CLI support list/add/update/remove accounts and selection policy.
Settings provides redacted account state and actions in the existing desktop design.
An operator can select an explicit account or use pool routing. Preserve authority
checks, current UI behavior, and backward compatibility for unconfigured users.

## Research and performance

Study (do not vendor) codex-lb at commit
`72630bbfeee9e3b0d9acfb95e2b1e706b8b0188e` (MIT), especially account eligibility,
soft cache affinity versus hard continuation ownership, and no replay after streaming.
Sources: https://github.com/Soju06/codex-lb and
https://soju06.github.io/codex-lb/routing/ . The native CLI docs and installed
Copilot 1.0.79 expose ACP, COPILOT_HOME, and token precedence
COPILOT_GITHUB_TOKEN > GH_TOKEN > GITHUB_TOKEN.

Session routing adds local selection and credential setup, not an extra model hop.
HTTP proxying adds local processing; remote proxies additionally add network RTT.
Account churn can lose caches and increase input usage, first-token latency, and
reconnect costs. Pooling improves eligible capacity/availability, not model generation
speed or an individual account's quota. Benchmark local overhead separately from live
model timing; report unmeasured quantities as unmeasured. Do not warm accounts by
spending quota implicitly.

## Acceptance

Tests cover Copilot discovery/handshake/permission/cancel/resume, profile isolation,
redaction, restart persistence, disabled/exhausted pools, pinning, concurrency,
proxy authentication/streaming/cooldown/no replay, owner RPCs, CLI and Settings.
Run targeted tests, workspace tests and required lint/build gates. Use local fixture
upstreams for routing and overhead; bounded real provider checks only where auth exists.
Publish exact limitations and verification with the PR; do not equate fixture success
with multi-account live acceptance.
