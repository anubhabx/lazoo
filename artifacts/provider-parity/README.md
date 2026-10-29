# Provider parity evidence

The completion record is `provider-parity-final-manifest.json`, with the UI-facing
contract in `docs/UI-PROVIDER-PARITY-HANDOFF.md`. Earlier `latest-result.json`,
feasibility notes and intermediate native results describe development probes;
they are not the final acceptance decision. Failed attempts are retained.

`acceptance_production.py` exercises the release daemon and CLI with a named
employee, a durable turn, the genuine native TUI, Lazoo MCP, a declined harmless
command, view close/reopen during approval, native history resume, and session
stop. Claude/Antigravity also exercise default trust notification, explicit
approval, the optional future managed-workspace policy, and policy reset.

```powershell
python -m pip install --target artifacts/provider-parity/.deps -r artifacts/provider-parity/requirements.txt
python artifacts/provider-parity/acceptance_production.py opencode
$env:LAZOO_LIVE_PROVIDER_PARITY = '1'
python artifacts/provider-parity/acceptance_production.py claude-code
python artifacts/provider-parity/acceptance_production.py antigravity
```

OpenCode uses only the local model fixture. Claude/Antigravity submit one bounded
prompt per invocation and require explicit live-test opt-in. No runner tests Grok.
Every invocation creates its own ignored `run-*` directory containing disposable
state, command outcomes, canonical events, and private native screen captures.
The checker renders VT output with pyte before asserting visible reply text;
native cursor redraws can split that text across separate byte chunks.
Only the small result manifests are intended as shareable acceptance evidence.

Run `scripts/gate.ps1` with one Cargo job and a dedicated output manifest. The
settled gate includes format, strict clippy, workspace tests, release build, and
all 39 ordinary acceptance scripts. Keep stress/resource measurements separate
from other workloads. The Codex local-fixture runner remains at
`artifacts/readiness/native-tui/acceptance_production.py`.

Historical files under `artifacts/acceptance` are backed up with hashes, fresh
gate manifests are copied here, then the historical files are restored exactly.
The local `baseline-acceptance` backup and raw native captures are intentionally
excluded from Git.
