# Copilot and account routing verification

Date: 2026-11-01 (Asia/Calcutta). Branch: `codex/copilot-account-routing`.
Stack base: `feat/desktop-ui-refresh` at `dc89861`, PR #41; that PR is based on #40.

## Scope

GitHub Copilot joins the governed ACP providers. Owner account controls cover isolated
native profiles and encrypted API credentials, balanced/pinned selection, durable resume
ownership, compatible streaming API pools, and explicit route verification. The support
matrix and setup steps are in [ACCOUNT-ROUTING.md](ACCOUNT-ROUTING.md).

Native subscriptions use native profile selection. HTTP pooling supports OpenAI Responses
and Anthropic Messages API credentials. It does not convert subscription OAuth into API
access. Copilot and Antigravity use native balancing/pinning; Grok's existing execution
limitations remain. Accepted conversations retain their account, including after a quota
failure; start a new session to select another eligible account.

## Checks

- Frontend: `npm run check` passed formatting, TypeScript, 46 tests, and production build.
  The existing large-bundle warning remains (approximately 960 kB before gzip).
- Visual inspection: Settings → Accounts in the integrated Slate UI, using the local
  scripted development bridge at 1920×889. Routing, verification, native/key creation,
  and empty-state controls were visible without overlap. This was a browser inspection,
  not evidence of a real authenticated account or native Tauri terminal interaction.
  [Inspected screenshot](../artifacts/accounts-settings.png).
- The complete repository gate passed **44 steps** on implementation commit `04ee33c`:
  formatting, strict workspace/all-target Clippy, test-fixture build, the entire workspace
  test suite, locked release build, and all 39 default acceptance scripts. The run took
  23.36 minutes. [Gate manifest](../artifacts/acceptance/copilot-accounts-gate.json).
- The gate's default exclusions remain the paid/live provider adapters/lifecycle/OpenCode
  checks, desktop-shell, and desktop-budget. The separate Copilot live acceptance is recorded
  below; native desktop interaction and desktop resource-budget acceptance are not claimed.

The gate ran with one Cargo build job, two test threads, incremental compilation disabled,
and dev/test debug symbols disabled to fit the available disk. Release settings were unchanged:

```powershell
$env:CARGO_BUILD_JOBS='1'
$env:CARGO_PROFILE_DEV_DEBUG='0'
$env:CARGO_PROFILE_TEST_DEBUG='0'
$env:CARGO_INCREMENTAL='0'
$env:RUST_TEST_THREADS='2'
pwsh -NoProfile -File scripts/gate.ps1 -OutputPath artifacts/acceptance/copilot-accounts-gate.json
```

The deterministic account tests exercise revision fencing, DPAPI/redacted transport,
balanced concurrent selection, profile isolation, restart affinity, disabled/tombstoned
owners, model restrictions, compatible failover, streaming without full-response buffering,
local proxy authentication, cancellation, and refusal to replay ambiguous/accepted requests.
Readiness tests cover account-policy/key invalidation, a changed credential during a
verification request, and preservation of managed verification across personal-login probes.
Managed OpenCode API tests require an explicit matching backend/model and restrict the
native provider configuration to the chosen backend, including default-endpoint keys.
Copilot fixtures cover ACP initialization, permission decisions, cancel, resume, MCP headers,
and rejection before session creation when HTTP MCP is unsupported.

## Installed Copilot

A no-model ACP initialization against the installed Copilot 1.0.79 binary returned
protocol version 1, HTTP/SSE MCP support, `loadSession: true`, and session close/list
capabilities. Initialization sends no model prompt and does not establish authentication
or subscription readiness. The adapter preserves that distinction in its public probe.

A separate, explicitly bounded live acceptance test then sent one text-only prompt through
the installed adapter using the existing native login. It returned `COPILOT_OK` in 15.44 s,
requested zero tools, and stopped the session. The observer declined tool permissions and
the turn had a 60-second deadline. This consumed one model turn; it verifies text execution,
not live MCP tool invocation, native resume, or a pool of multiple Copilot identities.

## Local proxy overhead

`measure_loopback_routing_overhead` used five warm-up requests and 30 measured samples
against the same local fixture upstream, directly and through the session proxy, in an
unoptimized test build. It reuses an established proxy; session startup and remote TLS/model
generation are excluded.

| Path | Median | p95 |
| --- | ---: | ---: |
| Direct fixture | 0.310 ms | 0.507 ms |
| Routed fixture | 1.661 ms | 2.024 ms |

Added median latency: **1.351 ms**. The run included local authentication, credential
decryption, routing, and observation writes. It sent no paid model requests. This is
not an estimate of real model latency, token charges, cache hit rates, or provider quotas.
Rotating fresh sessions can lose upstream prompt-cache reuse and raise first-token latency
or input usage. Native session selection adds no model-network proxy hop.

## Live boundaries

No live deployment with multiple paid upstream credentials was available for this test.
Real account login/refresh and provider-specific feature compatibility require the owner's
configured accounts. Adding an account never spends quota automatically; **Verify routing**
is an explicit one-prompt action. A successful verification establishes the selected route,
not every member of a pool.

The proxy deliberately forwards only supported model routes. File upload APIs, WebSockets,
arbitrary queries, and unsupported backend protocols are outside its contract. Continuation
IDs, signed/encrypted state, uploaded-file references, compact requests, partial streams,
and uncertain delivery prevent cross-account replay.
