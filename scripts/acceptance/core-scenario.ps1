param(
    [string]$OutputPath = ''
)

$ErrorActionPreference = 'Stop'
Set-StrictMode -Version Latest

$repoRoot = (Resolve-Path (Join-Path $PSScriptRoot '..\..')).Path
$cargoExe = (Get-Command cargo -CommandType Application -ErrorAction Stop |
    Select-Object -First 1).Source
$gitExe = (Get-Command git -CommandType Application -ErrorAction Stop |
    Select-Object -First 1).Source
$artifactDir = Join-Path $repoRoot 'artifacts\acceptance'
if ([string]::IsNullOrWhiteSpace($OutputPath)) {
    $OutputPath = Join-Path $artifactDir 'core-scenario-manifest.json'
}
$manifestPath = [IO.Path]::GetFullPath($OutputPath)
$manifestDirectory = Split-Path -Parent $manifestPath
New-Item -ItemType Directory -Force -Path $manifestDirectory | Out-Null

$startedAt = [DateTime]::UtcNow
$stopwatch = [Diagnostics.Stopwatch]::StartNew()
$commands = [Collections.Generic.List[object]]::new()
$manifestStatus = 'FAIL'

# Brief section 21.2, one row per step: what proves it, and how far. 'PASS' is this script's
# own run; 'PROVEN_ELSEWHERE' names the acceptance script that carries the step; 'PARTIAL' and
# 'NOT_COVERED' say so.
$steps = [ordered]@{
    step1 = [ordered]@{ text = 'The user configures Rhea as the lead and four peers across different providers'; status = 'PENDING'; evidence = 'core scenario story: Rhea leads; Mira and Nico on Codex, Oren on OpenCode, Pia on Grok Build, Quinn on Antigravity (four peers across different providers, plus a second Codex peer so the story can hold a sibling clarification)' }
    step2 = [ordered]@{ text = 'All employees begin dormant; one is disabled'; status = 'PENDING'; evidence = 'core scenario story: roster.get shows four dormant and Quinn disabled before any session starts' }
    step3 = [ordered]@{ text = 'The user starts a project conversation with Rhea'; status = 'PENDING'; evidence = 'core scenario story: the conversation is created in a registered project (a temporary git repository with a base commit) and led by Rhea' }
    step4 = [ordered]@{ text = 'Rhea receives a current roster and proposes a multi-part plan'; status = 'PENDING'; evidence = 'core scenario story: employees_list then two task_delegate calls inside the lead turn, and a third in a follow-up turn once the first plan finished' }
    step5 = [ordered]@{ text = 'Under ask mode, Lazoo presents precise delegation approvals'; status = 'PENDING'; evidence = 'core scenario story: both children are proposed, counted by the Inbox, and accepted by the user; provider permission approvals are approval-gate.ps1' }
    step6 = [ordered]@{ text = 'Approved employees activate into independent native sessions and editing worktrees'; status = 'PENDING'; evidence = 'core scenario story: each accepted task runs on a fresh session for Mira started inside its own retained git worktree of the project (never the canonical checkout), the session stops with its turn, and the next queued task wakes when she is free' }
    step7 = [ordered]@{ text = 'An employee asks a peer a direct clarification without routing through Rhea'; status = 'PENDING'; evidence = 'core scenario story: inside her dispatch turn Mira asks Nico through message_send and waits; Nico, waiting on her task in task_wait, receives the question in that wait and answers through message_reply; her wait returns his answer; the ledger holds the question on his task and the reply on hers; no peer turn reaches Rhea; peer-messaging.ps1 and coalesced-notices.ps1 prove the lead-recipient path and the idle-session delivery' }
    step8 = [ordered]@{ text = 'The disabled employee is not selected and cannot receive dispatch'; status = 'PENDING'; evidence = 'core scenario story: task_delegate to Quinn is refused LZ-TASK-ASSIGNEE-UNAVAILABLE and writes nothing' }
    step9 = [ordered]@{ text = 'One provider stalls after accepting work; Lazoo detects missing progress, retries safely, then raises a clean incident'; status = 'PENDING'; evidence = 'core scenario story: the third task dispatch turn stalls (the adapter progress deadline reported as a stalled provider error, as opencode_acceptance_without_progress_becomes_stalled proves for the real adapter), the turn is recorded retryable-failed, the task stays accepted for a bounded retry by the lead or the user and is never retried automatically, the worker session is stopped, and one LZ-SESSION-STALLED incident on the task is raised and promoted at once' }
    step10 = [ordered]@{ text = 'The threshold notifies Rhea, allows bounded remediation, and escalates to the user when unresolved'; status = 'PENDING'; evidence = 'core scenario story: the stall on the queued third task opens the lead window, the lead cancels it in its notice turn, the stall recurs, the sweep escalates to the user, and the user acknowledges it from the Inbox' }
    step11 = [ordered]@{ text = 'Lazoo restarts during an in-flight message and reconciles without losing or duplicating the logical task'; status = 'PENDING'; evidence = 'core scenario story restarts with the prior boot owner-lost and every task kept once; the in-flight attempt is message_restart_reconciles_an_uncertain_attempt in collaboration-bus.ps1 and native-session-ownership.ps1' }
    step12 = [ordered]@{ text = 'Results include commits, verification, task links, messages, incidents, and concise memory records'; status = 'PENDING'; evidence = 'core scenario story: the first worker reports completion through task_report with commit and verification evidence, which is the task''s durable evidence through task.show, and the result notice links the task; messages and incidents are ledgers of their own (result-delivery.ps1, minimum-incidents.ps1); on the restarted daemon the completed task''s dispatch turn is archived through memory.archive, a dedicated dormant Codex summary employee opted in through memory.worker.configure summarizes it in one fresh read-only turn, and the typed memory record citing that turn and the archive''s source hash is read back through memory.search by taskId (the summary output is scripted on the fake transport: the worker pipeline is proven, model summary quality is not)' }
    step13 = [ordered]@{ text = 'Integration follows configured authority and never overwrites dirty user work'; status = 'PENDING'; evidence = 'core scenario story: every task runs under the ask authority the user configured, each worktree starts from the committed base, and the dirty edit in the canonical checkout is untouched after both dispatches; project_task_dispatch_uses_an_isolated_worktree_and_preserves_dirty_user_files (automatic-task-dispatch.ps1) proves the same with a running provider turn' }
    step14 = [ordered]@{ text = 'Employees return to idle and suspend after policy timeout'; status = 'PENDING'; evidence = 'core scenario story: Mira idles after warm and is suspended once her one-minute timeout passes' }
    step15 = [ordered]@{ text = 'Lazoo remains inside its measured resource budget'; status = 'PROVEN_ELSEWHERE'; evidence = 'resource-budget.ps1 (idle and routing windows only)' }
}
$evidence = [ordered]@{
    scenarioStory = $false
    worktreePreservation = $false
    restartReconciliation = $false
    schemaFifteen = $false
    providerSmokeAbsent = $true
    externalModelUseAbsent = $true
    uiChangesAbsent = $false
}

function Invoke-Test([string]$Package, [string]$Target, [string]$Name) {
    $arguments = @('test', '-p', $Package, '--test', $Target, $Name, '--locked', '--', '--exact')
    $timer = [Diagnostics.Stopwatch]::StartNew()
    & $cargoExe @arguments
    $exitCode = $LASTEXITCODE
    $timer.Stop()
    $commands.Add([ordered]@{
        command = "cargo $($arguments -join ' ')"
        exitCode = $exitCode
        durationMs = [long]$timer.ElapsedMilliseconds
    })
    if ($exitCode -ne 0) {
        throw "cargo test failed: $Name"
    }
}

try {
    Push-Location $repoRoot
    Invoke-Test 'lazood' 'daemon' 'the_core_scenario_runs_end_to_end_on_the_deterministic_provider'
    $evidence.scenarioStory = $true
    foreach ($key in @('step1', 'step2', 'step3', 'step4', 'step5', 'step6', 'step7', 'step8', 'step9', 'step10', 'step11', 'step12', 'step13', 'step14')) {
        $steps[$key].status = 'PASS'
    }

    Invoke-Test 'lazood' 'daemon' 'project_task_dispatch_uses_an_isolated_worktree_and_preserves_dirty_user_files'
    $evidence.worktreePreservation = $true

    Invoke-Test 'lazood' 'daemon' 'message_restart_reconciles_an_uncertain_attempt'
    $evidence.restartReconciliation = $true

    Invoke-Test 'lazoo-store' 'store' 'store_enables_wal_foreign_keys_and_schema_version'
    $evidence.schemaFifteen = $true

    & $gitExe diff --quiet -- apps/lazoo-desktop
    $unstagedExit = $LASTEXITCODE
    & $gitExe diff --cached --quiet -- apps/lazoo-desktop
    $stagedExit = $LASTEXITCODE
    $commands.Add([ordered]@{
        command = 'git diff --quiet and git diff --cached --quiet -- apps/lazoo-desktop'
        exitCode = [Math]::Max($unstagedExit, $stagedExit)
        durationMs = $null
    })
    if (($unstagedExit -ne 0) -or ($stagedExit -ne 0)) {
        throw 'desktop UI changes are outside the core scenario slice'
    }
    $evidence.uiChangesAbsent = $true
    $manifestStatus = 'PASS'
} finally {
    Pop-Location
    $stopwatch.Stop()
    $manifest = [ordered]@{
        status = $manifestStatus
        schemaVersion = 19
        mode = 'deterministic-fake-transport'
        startedAt = $startedAt.ToString('o')
        durationMs = [long]$stopwatch.ElapsedMilliseconds
        commands = @($commands)
        steps = $steps
        evidence = $evidence
        boundaries = [ordered]@{
            providerSmoke = 'NOT_RUN'
            externalModelUse = 'NOT_RUN'
            desktopUi = 'UNCHANGED'
            schemaMigration = 'NONE'
            providers = 'FAKE_TRANSPORT_FOR_EVERY_PROVIDER'
            liveScenario = 'NOT_RUN_REQUIRES_EXPLICIT_AUTHORIZATION'
        }
    }
    $manifest | ConvertTo-Json -Depth 8 | Set-Content -LiteralPath $manifestPath -Encoding utf8
}

Write-Host "PASS manifest: $manifestPath"
