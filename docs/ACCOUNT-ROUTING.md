# Provider accounts and GitHub Copilot

Lazoo can choose an isolated account when starting a native provider session and can
route compatible API traffic through a local credential pool. Existing installations
keep their current provider configuration until the owner changes a routing policy.

## Setup

Open **Settings → Accounts**. Add a native profile or an API credential, then choose
the provider's routing policy. Native profiles get their own configuration directory
below Lazoo's data root. **Sign in** opens that provider's own login command in a
writable terminal. Complete sign-in there before enabling pooled native sessions.
Use **Verify routing** after choosing the policy. It sends one short model prompt,
consumes quota, and establishes launcher readiness for that route. Supply a model if
the account restricts models. OpenCode API accounts always require `backend/model`,
so a default/free provider cannot accidentally verify a different route. Verification is never
run automatically when adding an account or opening Settings. Credential and routing
changes invalidate earlier readiness; a personal login probe cannot substitute for it.

API secrets are submitted once, protected with Windows DPAPI for the current user,
and omitted from account lists and audit events. **Rotate key** replaces the secret
using the account's current revision. Password fields clear after submission;
plaintext credentials are not saved in browser storage or placed in process arguments.

| Policy | Behavior |
| --- | --- |
| Use existing behavior | Use the provider's preexisting environment and login. |
| Balance accounts | Select the least-selected eligible account for a new native session. |
| Pin an account | Select the specified account for new sessions. |
| Pool API credentials | Start a session-scoped loopback proxy and select an eligible API credential. |

Eligibility checks enabled/deleted state, model restrictions and known API cooldowns.
A configured pool with no eligible member fails with `LZ-ACCOUNT`; it does not fall
back to a personal login. Native profile eligibility does not prove that its login or
subscription is ready: authentication remains the native CLI's responsibility.

Policy changes affect new sessions. Existing/resumed sessions retain their account,
profile directory, and native history. Disabling/removing their account prevents a
future resume. A running API proxy checks account availability before each request.
Already-running direct native processes retain their launch environment until stopped.

## Provider capabilities

| Provider | Native profile isolation | Direct key/token | HTTP API pool |
| --- | --- | --- | --- |
| Codex | `CODEX_HOME` | `OPENAI_API_KEY` | Responses, including compact |
| Claude Code | `CLAUDE_CONFIG_DIR` | `ANTHROPIC_API_KEY`; CLI also accepts `CLAUDE_CODE_OAUTH_TOKEN` | Anthropic API keys; Messages, beta Messages and token counts |
| GitHub Copilot | `COPILOT_HOME`, isolated `GH_CONFIG_DIR` | `COPILOT_GITHUB_TOKEN` | No; use native session balancing/pinning |
| OpenCode | Isolated home and XDG config/data/state/cache | OpenAI, Anthropic, Google and OpenRouter keys | `openai` Responses and `anthropic` Messages backends |
| Antigravity | Isolated home and Windows app-data directories | Native login | No; use native session balancing/pinning |

Grok Build remains subject to its existing adapter limitations; this feature does not
admit it into governed execution. Native subscriptions are routed as native sessions.
Lazoo does not extract subscription OAuth tokens, imitate private subscription HTTP
endpoints, or convert subscriptions into API-key quotas.

For OpenCode, use a `backend/model` employee selection matching the configured backend.
Model restrictions accept either the bare upstream model ID or that qualified name.
Managed API-key sessions enable only that backend, including the native takeover terminal;
native profiles keep their own provider choices. This prevents a default or auxiliary
model from escaping the selected credential route.
The native CLI and its own account store continue to own OAuth refresh. Lazoo does not
refresh copied OAuth credentials itself.

Copilot uses the CLI's stdio ACP interface, with Lazoo's permission observer and a
session-scoped authenticated MCP bridge. It supports model/effort selection, streaming,
cancel, and ACP session loading when advertised by the installed version. Version
and ACP probes do not send a model prompt and do not establish subscription readiness.
There is no claimed concurrent Copilot native TUI view. Suspended sessions can be
opened through the native resume command; existing providers keep their actual TUIs.

## API routing and safety

The proxy listens only on loopback with a random per-session access token. Upstream
keys are substituted inside the daemon, never forwarded from the caller's headers.
HTTPS is required for configured upstreams; HTTP is allowed only for loopback fixtures
or a deliberately configured local gateway. Redirects are disabled. Request bodies
are capped at 8 MiB; responses stream without full-response buffering.

New sessions balance across eligible credentials. Before a session's first accepted
model request, an explicit 401, 403 or 429 can try one compatible replacement. Protocol,
backend, credential type and model eligibility must match. Transport failures are
not replayed because delivery is uncertain. Accepted sessions, resumed sessions,
previous response IDs, uploaded file references, and compact requests never rotate
silently to a different credential. Partial streams and client cancellation do not
trigger a replay. The native profile owner and the upstream API owner are persisted
separately so early failover does not lose native transcript history.

Authentication failures cool down an API credential for one hour. Quota responses use
numeric `Retry-After`, bounded to 1–86,400 seconds (60 seconds if missing). Server
errors have a short bounded cooldown. Replacing a secret clears its cooldown. Metrics
are request count, failures, last HTTP status, and time to response headers; they are
not remaining subscription quota or billable-token accounting.

Use an API URL prefix such as `https://gateway.example/v1` for custom endpoints.
Anthropic also accepts a server root; Lazoo normalizes the `/v1` difference between
Claude Code's SDK and OpenCode's SDK. Arbitrary routes, WebSockets, uploads, and
arbitrary query strings are not forwarded. Beta Anthropic requests preserve
`?beta=true`. API pools cannot serve a provider feature that needs another route.

## CLI

List redacted records and policy revisions:

```powershell
lazooctl account list
```

Create a native profile (the response supplies its ID/revision):

```powershell
lazooctl account put --provider github-copilot --label Work
lazooctl account login <account-id> --expected-revision 1
lazooctl account policy github-copilot balanced --expected-revision 0
lazooctl provider smoke github-copilot --prompt "Reply OK without using tools"
```

`account login` returns a terminal record. The desktop displays it automatically;
CLI users can operate it through the existing terminal commands. It does not silently
log in or replace the owner's personal provider credentials.

For API credentials, supply the secret on standard input from a password manager or
other private input source. Do not place a literal secret in command history:

```text
<private secret source> | lazooctl account put --provider codex --label Work --credential-environment OPENAI_API_KEY --api-protocol responses --key-stdin
lazooctl account policy codex api-pool --expected-revision 0
```

For an OpenCode Anthropic pool member add `--backend-provider anthropic`, use
`--credential-environment ANTHROPIC_API_KEY` and `--api-protocol anthropic`.
For an OpenCode OpenAI member use `--backend-provider openai` with Responses.
Google/OpenRouter keys support direct session routing, without `--api-protocol`.

`account put --id <id> --expected-revision <revision>` updates an existing record.
Keep its immutable identity fields (provider, credential type, profile directory,
endpoint, API protocol and backend) unchanged. Label, model restrictions, enabled state
and secret can change. `account remove <id> --expected-revision <revision>` clears the
protected key and preserves an ownership tombstone. Native profile files remain on
disk so removing an account does not delete provider history.

## Performance and research

Native session selection adds a local database transaction and private environment
setup, without an additional network hop. The API pool adds a loopback HTTP hop,
credential decryption, and a small metrics transaction. Request bodies and streamed
response bytes are preserved. This is an availability/capacity feature; it cannot
increase an individual subscription's quota or make model generation faster.
Keys that share an upstream quota still share it after pooling.

Moving fresh sessions across accounts can reduce prompt-cache reuse, increasing
input usage and first-token latency. Keeping accepted sessions pinned avoids repeated
account churn within a conversation. Adding a remote gateway would also add its network
latency. Real model timing and billing depend on the upstream and cannot be inferred
from a loopback fixture benchmark.

The design was informed by [codex-lb](https://github.com/Soju06/codex-lb) at
`72630bbfeee9e3b0d9acfb95e2b1e706b8b0188e` (MIT), especially the distinction between
[soft cache affinity and hard continuation ownership](https://soju06.github.io/codex-lb/routing/).
No codex-lb source was vendored.

Provider configuration references:

- [Codex custom provider configuration](https://developers.openai.com/siwc/token-sharing-open-source/codex-app-server)
- [Anthropic SDK base URL](https://github.com/anthropics/anthropic-sdk-typescript/blob/main/src/client.ts)
- [Anthropic SDK Messages and beta paths](https://github.com/anthropics/anthropic-sdk-typescript/blob/main/api.md)
- [AI SDK Anthropic URL prefix](https://github.com/vercel/ai/blob/main/content/providers/01-ai-sdk-providers/05-anthropic.mdx)
- [OpenCode provider allowlist and model configuration](https://github.com/anomalyco/opencode/blob/v1.18.29/packages/web/src/content/docs/config.mdx)

See the [verification report](ACCOUNT-ROUTING-VERIFICATION.md) for evidence and measured
overhead. Fixture tests establish routing behavior; they do not prove every native login
or every upstream subscription has been accepted in a live multi-account deployment.
