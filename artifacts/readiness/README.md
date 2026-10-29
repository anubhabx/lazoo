# UI backend readiness evidence

The authoritative current result is [ui-backend-final-manifest.json](ui-backend-final-manifest.json),
verified on source `feab4d556b3424d3b9b0b7ec9e155f62434b8a95` on 2026-10-29.
Subsequent commits only preserve evidence and update documentation.

- [Final gate](gate-native-final-manifest.json): all 44 steps passed, including
  39 acceptance scripts and 607 workspace tests (14 intentionally ignored).
- [Release native TUI](native-tui/release-evidence/result.json): all 13 checks
  passed with real Codex 0.165.0-alpha.1.2 and a local Responses fixture.
  The same directory preserves native screen bytes/text, session and turn records.
- [CLI stress](acceptance/cli-stress-a186aaed80f44e7a921529eca178eacd.json):
  all 2,048 calls passed at concurrency 16 without stderr warnings.
- [Acceptance preservation](acceptance-preservation.json): original-to-copy hashes.
  Historical tracked manifests were restored after copying the current evidence.
- [Codex configuration compatibility](codex-config-0.165/manifest.json): actual
  installed native parser evidence, with configuration lifecycle covered by the gate.

`native-tui/acceptance_production.py release` reruns the production native flow.
Its disposable run directories are ignored; the selected release evidence above
is versioned. Full gate/native/stress logs are retained locally as ignored `.log`
files. The binary hashes in the native and stress manifests match.

Other root-level gate manifests document earlier failed or incomplete attempts;
they are not the final result. In particular, `gate-native-before-config-upgrade`
records the removed-installation failure before the verified version update,
and `gate-native-lint-initial` records findings fixed before the final gate.
`attempts/acceptance` retains the earlier core-services result.

This establishes a Codex UI foundation, not full core v1 completion. It does not
verify current external model/account availability, implement the frontend, or
complete non-Codex collaboration/native views and remaining product performance.
