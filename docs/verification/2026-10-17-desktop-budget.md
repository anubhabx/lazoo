# Desktop resource budget verification

Date completed: 2026-10-17 IST

Branch: `feat/core-v1-completion`

Base: `1072025`

Product version: `0.1.0`

Database schema: 19 (no migration)

Scope: one acceptance script, `scripts/acceptance/desktop-budget.ps1`, that measures the brief's section 20.1 idle and routing targets for "Lazoo itself" with the desktop running: keeper, daemon, the desktop process, and every WebView2 process that descends from it. It follows `2026-10-13-resource-budget.md`, which measured keeper plus daemon only and listed the desktop's own share as a retained boundary. No production code and nothing under `apps/lazoo-desktop` changed.

## Outcome

FAIL, on one target: idle memory. CPU passes both targets with a wide margin.

The script exits 1 and still writes `artifacts/acceptance/desktop-budget-manifest.json` (status `FAIL`, schema 19, `mode: real-binaries-with-desktop`). Every other gate passes, including cleanup of every process the run started.

Measured on the owner's laptop (12 logical processors), final run, 60-second windows sampled at about one second, nothing else of Lazoo's running, no provider and no model:

| Target | Measured | Limit | Result |
|---|---:|---:|---|
| Idle CPU, worst rolling 30 s mean | 0.076 % of machine (window mean 0.062 %, peak 1 s sample 0.83 %) | under 1 % | PASS |
| Routing CPU, worst rolling 30 s mean | 0.307 % (window mean 0.284 %, peak 1 s sample 1.02 %) | under 5 % | PASS |
| Idle memory, peak summed working set | 434.2 MiB (window mean 427.1 MiB) | under 350 MiB | FAIL |
| Active-core memory, peak summed working set during routing | 434.8 MiB | under 750 MiB | PASS |

Where the CPU goes (share of total machine capacity, final run):

| Process group | Idle 60 s | Routing 60 s |
|---|---:|---:|
| Daemon | 0.000 % (0 ms) | 0.249 % (1797 ms) |
| Desktop process | 0.007 % (47 ms) | 0.000 % (0 ms) |
| WebView2 descendants (6 processes seen) | 0.055 % (391 ms) | 0.035 % (249 ms) |
| Keeper | 0 ms | 0 ms |

The first complete run, on the same binary, agreed: idle 0.159 % and 454.1 MiB, routing 0.375 % and 449.8 MiB. Memory sits roughly 80 to 105 MiB over the idle target in both runs, so the failure is not noise.

### Reading the memory failure

`WorkingSet64` summed across the desktop and its WebView2 processes counts pages that several WebView2 processes share (runtime DLL images and similar) once per process, so 434 MiB is an upper bound on resident memory, not a unique figure. The same samples peaked at 191.8 MiB of private bytes at idle (192.7 MiB during routing). Private bytes are committed private memory, not resident memory, so they are not a substitute for the working-set reading; they only show that the true unique footprint is well below the summed working set. The gate uses the summed working set because the brief says working-set memory and a shared-aware measure was not built. Keeper plus daemon are about 13 MiB of daemon working set plus the keeper; essentially all of the overshoot is the WebView2 process tree. A per-group working-set split was not recorded, so that last sentence rests on the daemon's own `system resources` reading (13.1 to 13.7 MiB) and the process count, not on a measured split.

The decision on whether the target should be re-read as a shared-aware figure (private working set plus shared pages once), or the desktop's memory reduced, belongs to the owner and the UI owner. This slice reports the number it was asked for.

### The daemon's own accounting, recorded beside the process-table figures

`lazooctl system resources` and `system budget` were read before the idle window, after it, and after the routing window; all three are in the manifest under `daemonAccounting`. The daemon's infrastructure job excludes the desktop and provider trees (`desktopIncluded: false`) but does enroll every `lazooctl` process that ran. The informational `infrastructureJobIncludingCli` block therefore shows:

| Window | Job CPU (keeper, daemon, every CLI) | Share of machine | Processes enrolled |
|---|---:|---:|---:|
| Idle 60 s | 31 ms | 0.004 % | 2 |
| Routing 60 s | 54,953 ms | 7.58 % | 3,723 |

The daemon's own rolling 30-second reading at the end of the routing window was 7 percent, and 0 percent after idle. That 7 percent is above the 5 percent routing target. It is not what the gate measures: about 97 percent of that CPU is the roughly 3,720 short-lived `lazooctl` processes that the script runs back to back (about 15 ms of CPU each, about 62 calls a second) as its load generator, which is far more control traffic than normal routing. The gate follows `resource-budget.ps1` and does not count the load generator, but the brief lists the CLI while active as part of "Lazoo itself", so the 7 percent is reported rather than hidden. A routing workload sized to realistic call rates, or an agreed rule for how the load generator is attributed, is a follow-up decision.

## Design notes

| Decision | Reason |
|---|---|
| Count keeper, daemon, desktop, and all descendants of the desktop | Section 20 puts the desktop in "Lazoo itself". Tauri's WebView2 browser, renderer, GPU, and utility processes are children of the desktop process. |
| Attribute WebView2 processes by parent PID, never by image name | The laptop runs other `msedgewebview2.exe` processes (Edge, other apps). The roster is rebuilt from the process table every third sample from the desktop's PID down, so only this desktop's tree is counted and processes that appear or exit mid-window are picked up. |
| CPU from per-process CPU time over wall-clock time over logical processors | Same definition as `resource-budget.ps1` and the brief (share of total machine capacity); unaffected by other load. A process that exits between samples is still read from its held handle, and a process that appears mid-window counts from zero. |
| Per-second sampling with a rolling mean | The brief asks for rolling 30-second and 5-minute windows, not a single sample. The CPU gate is the worst rolling 30-second mean (and the worst rolling 5-minute mean when the window is at least 300 s). The peak 1-second sample is recorded but not gated, because the desktop refreshes every 10 seconds. |
| Default 60-second windows, `-FullWindows` for 300 seconds | Sixty seconds gives two rolling 30-second windows in each state and keeps the default run near 2.5 minutes. The 5-minute rolling figure is `null` with a note in the default run rather than a guess. `-FullWindows` runs idle and routing at 300 seconds each (about 12 minutes) and fills it. It was not run in this slice. |
| 20-second settle after the UI is ready | WebView2 start-up and first render are not idle. The settle is a manifest field. |
| Readiness by the same evidence as `desktop-shell.ps1` | The window has the expected title and UI Automation sees "Control plane steady", so the UI has fetched a snapshot from the connected daemon. This is one UI Automation probe before the settle; it can switch WebView2 accessibility on, which makes the figures slightly conservative. |
| Routing is `resource-budget.ps1`'s activity | `status`, `project list`, `events list`, `inbox counts`, `roster show` in a loop plus one project registration; fake transport, no model. The desktop stays open and keeps its 10-second refresh. |
| Idle memory is gated on the peak, not the mean | A budget ceiling should hold at its highest sample. |
| Active-core memory target (750 MiB) is applied to the routing window | There is no larger workload here, and the routing window is the busiest the gate drives. |

## Commands

All run from `C:\workspace\Orchestrator` on `feat/core-v1-completion`, with `CARGO_TARGET_DIR=C:\workspace\Orchestrator\target`.

| Command | Exit | Evidence |
|---|---:|---|
| `pwsh -NoProfile -File scripts\acceptance\desktop-budget.ps1 -SkipBuild` (first complete run) | 1 | FAIL on idle memory only; 144 s. |
| the same, after adding the informational job-inclusive block | 1 | Failed inside the script (a timestamp conversion defect in the new block, not a budget result); the defect was fixed. |
| the same, final | 1 | FAIL on idle memory only; 143 s; the numbers above. |

No cargo command was run, and no npm command: the prebuilt desktop binary and `apps/lazoo-desktop/ui/dist` already existed, so the default `-SkipBuild` path was used. `-Build` is available and runs `npm --prefix apps/lazoo-desktop/ui ci`, `npm --prefix apps/lazoo-desktop/ui run build`, and `cargo build -p lazoo-desktop --release --locked`; it was not exercised.

## Boundary evidence

- Nothing under `apps/lazoo-desktop` was edited (`desktopUi: UNCHANGED` in the manifest). The only files this slice adds are `scripts/acceptance/desktop-budget.ps1`, this document, and the manifest.
- Desktop binary measured: `target\release\lazoo-desktop.exe`, built 2026-10-17 14:05, SHA-256 `9df5ef807cf381ae8ac2a42a167b7c62b14b4e9cb8df2e9e7990ba62e3a1464f`.
- UI assets were not built by this run (`uiAssetsBuiltByThisRun: false`). The packaged `ui/dist/index.html` has SHA-256 `9ae6f96364382d6fc6140b245a3a5bd54c0707529300772628e9f95811fb8687`. It was not checked against the assets embedded in the binary, which was built before this session.
- `modelUse: NONE`: no provider or model started, and `liveSessions` was 0 at the end. No limit or throttle was set (`enforcement: NONE`).
- The script uses a disposable `LAZOO_DATA_ROOT` and temporary repositories under the temp directory, and its cleanup gate proves that every keeper, daemon, desktop, and WebView2 process it tracked is gone.

## Gate exclusion for the integrator

`desktop-budget.ps1` is a multi-minute measurement that opens a window, so it must sit beside `desktop-shell` in the exclusions: in `scripts/gate.ps1`,

```
$excluded = @('provider-adapters', 'provider-live-lifecycle', 'opencode-governed-live', 'desktop-shell', 'desktop-budget')
```

and the same literal pinned in `gate_runner_runs_the_whole_gate_and_excludes_only_the_live_and_ui_scripts` in `apps/lazood/tests/daemon.rs`. This slice did not edit either file. The working tree already carries exactly that change in both files, made outside this slice.

## Retained boundaries

- Not measured here: failure storm, memory indexing, terminal streaming, and voice scenarios; the `lazooctl` load generator in the gated figures (reported separately, above); a per-process-group memory split; a shared-aware memory figure; the 5-minute rolling windows (only with `-FullWindows`, not run).
- The memory failure is an open decision for the owner and the UI owner, as described above. The job-inclusive 7 percent routing reading is an open attribution question.
- This is one measurement on the owner's laptop. It is not a claim for other hardware, other WebView2 versions, or a machine under other load.
- Section 20.2 enforcement and everything else already listed as retained in `2026-10-13-resource-budget.md` remain separate slices.
