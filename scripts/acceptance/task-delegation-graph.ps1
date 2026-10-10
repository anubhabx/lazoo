param(
    [switch]$SkipBuild,
    [string]$OutputPath = ''
)

$ErrorActionPreference = 'Stop'
Set-StrictMode -Version Latest

function Assert-Equal($Actual, $Expected, [string]$Label) {
    if ($Actual -ne $Expected) {
        throw "$Label expected '$Expected' but found '$Actual'"
    }
}

function Assert-True([bool]$Condition, [string]$Label) {
    if (-not $Condition) {
        throw "$Label expected true"
    }
}

function Assert-Contains([string]$Actual, [string]$Expected, [string]$Label) {
    if (-not $Actual.Contains($Expected)) {
        throw "$Label expected '$Expected' in '$Actual'"
    }
}

function Set-DataRoot([string]$Value) {
    $previous = $env:LAZOO_DATA_ROOT
    $env:LAZOO_DATA_ROOT = $Value
    return $previous
}

function Restore-DataRoot($Previous) {
    if ($null -eq $Previous) {
        Remove-Item Env:LAZOO_DATA_ROOT -ErrorAction SilentlyContinue
    } else {
        $env:LAZOO_DATA_ROOT = $Previous
    }
}

function Add-Command([string]$Command, [int]$ExitCode, $DurationMs) {
    $script:commands.Add([ordered]@{
        command = $Command
        exitCode = $ExitCode
        durationMs = if ($null -eq $DurationMs) { $null } else { [long]$DurationMs }
    })
}

function Invoke-Json([string[]]$Arguments) {
    $previousRoot = Set-DataRoot $script:dataRoot
    $previousPreference = $ErrorActionPreference
    try {
        $ErrorActionPreference = 'Continue'
        $timer = [Diagnostics.Stopwatch]::StartNew()
        $captured = @(& $script:ctlExe --json @Arguments 2>&1)
        $exitCode = $LASTEXITCODE
        $timer.Stop()
        Add-Command "lazooctl --json $($Arguments -join ' ')" $exitCode $timer.ElapsedMilliseconds
        if ($exitCode -ne 0) {
            throw "lazooctl $($Arguments -join ' ') exited $exitCode with: $($captured -join [Environment]::NewLine)"
        }
        return (($captured -join [Environment]::NewLine) | ConvertFrom-Json)
    } finally {
        $ErrorActionPreference = $previousPreference
        Restore-DataRoot $previousRoot
    }
}

function Invoke-ExpectedFailure([int]$ExpectedExit, [string]$ExpectedCode, [string[]]$Arguments) {
    $previousRoot = Set-DataRoot $script:dataRoot
    $previousPreference = $ErrorActionPreference
    try {
        $ErrorActionPreference = 'Continue'
        $timer = [Diagnostics.Stopwatch]::StartNew()
        $captured = @(& $script:ctlExe --json @Arguments 2>&1)
        $exitCode = $LASTEXITCODE
        $timer.Stop()
        Add-Command "lazooctl --json $($Arguments -join ' ')" $exitCode $timer.ElapsedMilliseconds
        $output = $captured -join [Environment]::NewLine
        Assert-Equal $exitCode $ExpectedExit "expected failure: $($Arguments -join ' ')"
        Assert-Contains $output $ExpectedCode "expected failure code"
        return $output
    } finally {
        $ErrorActionPreference = $previousPreference
        Restore-DataRoot $previousRoot
    }
}

function Start-Daemon {
    $startInfo = [Diagnostics.ProcessStartInfo]::new()
    $startInfo.FileName = $script:daemonExe
    $startInfo.UseShellExecute = $false
    $startInfo.CreateNoWindow = $true
    $startInfo.Environment['LAZOO_DATA_ROOT'] = $script:dataRoot
    $startInfo.Environment['PATH'] = "$script:fixtureDir;$env:PATH"
    $startInfo.Environment['LAZOO_TURN_FIXTURE_LOG'] = $script:fixtureLog
    $startInfo.Environment['LAZOO_TURN_FIXTURE_THREAD_ID'] = "fixture-thread-$script:runId"
    foreach ($outerCodexVariable in @('CODEX_THREAD_ID', 'CODEX_SESSION_ID')) {
        [void]$startInfo.Environment.Remove($outerCodexVariable)
    }
    $process = [Diagnostics.Process]::new()
    $process.StartInfo = $startInfo
    Assert-True $process.Start() 'daemon process start'
    $script:createdProcesses.Add($process)
    return $process
}

function Stop-Daemon([Diagnostics.Process]$Process) {
    Invoke-Json @('system', 'shutdown') | Out-Null
    if (-not $Process.WaitForExit(15000)) {
        throw "daemon $($Process.Id) did not stop after planned shutdown"
    }
    Assert-Equal $Process.ExitCode 0 'planned daemon exit code'
}

function Wait-Status([string]$DifferentBootId = '', [int]$TimeoutSeconds = 20) {
    $deadline = [DateTime]::UtcNow.AddSeconds($TimeoutSeconds)
    $lastError = ''
    while ([DateTime]::UtcNow -lt $deadline) {
        try {
            $status = Invoke-Json @('status')
            if (($DifferentBootId -eq '') -or ([string]$status.bootId -ne $DifferentBootId)) {
                return $status
            }
        } catch {
            $lastError = $_.Exception.Message
        }
        Start-Sleep -Milliseconds 100
    }
    throw "daemon did not become ready: $lastError"
}

function Wait-TaskState([string]$TaskId, [string[]]$States, [int]$TimeoutSeconds = 20) {
    $deadline = [DateTime]::UtcNow.AddSeconds($TimeoutSeconds)
    $last = $null
    while ([DateTime]::UtcNow -lt $deadline) {
        $last = Invoke-Json @('task', 'show', $TaskId)
        if ($States -contains [string]$last.task.state) {
            return $last
        }
        Start-Sleep -Milliseconds 100
    }
    throw "task $TaskId did not reach $($States -join ','); last state was $($last.task.state)"
}

function Start-JsonCommand([string]$Name, [string[]]$Arguments) {
    $stdoutPath = Join-Path $script:runRoot "$Name.stdout.json"
    $stderrPath = Join-Path $script:runRoot "$Name.stderr.txt"
    $previousRoot = Set-DataRoot $script:dataRoot
    try {
        $process = Start-Process -FilePath $script:ctlExe -ArgumentList (@('--json') + $Arguments) `
            -RedirectStandardOutput $stdoutPath -RedirectStandardError $stderrPath `
            -PassThru -WindowStyle Hidden
        $script:createdProcesses.Add($process)
        return [pscustomobject]@{
            Process = $process
            StdoutPath = $stdoutPath
            StderrPath = $stderrPath
            Arguments = $Arguments
        }
    } finally {
        Restore-DataRoot $previousRoot
    }
}

function Complete-JsonCommand($Command, [int]$TimeoutSeconds = 20) {
    if (-not $Command.Process.WaitForExit($TimeoutSeconds * 1000)) {
        throw "background command did not finish: $($Command.Arguments -join ' ')"
    }
    $Command.Process.WaitForExit()
    $Command.Process.Refresh()
    $exitCode = [int]$Command.Process.ExitCode
    $stdout = if (Test-Path -LiteralPath $Command.StdoutPath) {
        Get-Content -LiteralPath $Command.StdoutPath -Raw
    } else { '' }
    $stderr = if (Test-Path -LiteralPath $Command.StderrPath) {
        Get-Content -LiteralPath $Command.StderrPath -Raw
    } else { '' }
    Add-Command "lazooctl --json $($Command.Arguments -join ' ')" $exitCode $null
    if ($exitCode -ne 0) {
        throw "background lazooctl exited ${exitCode}: $stderr"
    }
    return ($stdout | ConvertFrom-Json)
}

function Complete-InterruptedCommand($Command, [int]$TimeoutSeconds = 20) {
    if (-not $Command.Process.WaitForExit($TimeoutSeconds * 1000)) {
        try { $Command.Process.Kill() } catch [InvalidOperationException] {}
        $Command.Process.WaitForExit(5000) | Out-Null
    }
    $Command.Process.Refresh()
    $exitCode = if ($Command.Process.HasExited) { [int]$Command.Process.ExitCode } else { 137 }
    Add-Command "lazooctl --json $($Command.Arguments -join ' ')" $exitCode $null
}

function Get-TaskByObjective([object[]]$Tasks, [string]$Objective) {
    $matching = @($Tasks | Where-Object { [string]$_.objective -eq $Objective })
    Assert-Equal $matching.Count 1 "task objective $Objective"
    return $matching[0]
}

function Invoke-Delegate(
    [string]$ParentId,
    [string]$ActorEmployee,
    [string]$ActorSession,
    [string]$AssignedEmployee,
    [string]$Objective,
    [string]$Authority,
    [long]$RosterRevision,
    [string]$Key
) {
    return Invoke-Json @(
        'task', 'delegate', $ParentId,
        '--actor-employee', $ActorEmployee,
        '--actor-session', $ActorSession,
        '--assigned-employee', $AssignedEmployee,
        '--objective', $Objective,
        '--requested-authority', $Authority,
        '--roster-revision', ([string]$RosterRevision),
        '--idempotency-key', $Key
    )
}

$repoRoot = (Resolve-Path (Join-Path $PSScriptRoot '..\..')).Path
$cargoExe = (Get-Command cargo -CommandType Application -ErrorAction Stop |
    Select-Object -First 1).Source
$releaseDir = Join-Path $repoRoot 'target\release'
$script:daemonExe = Join-Path $releaseDir 'lazood.exe'
$script:ctlExe = Join-Path $releaseDir 'lazooctl.exe'
$fixtureExe = Join-Path $releaseDir 'lazoo-turn-transport-fixture.exe'
$artifactDir = Join-Path $repoRoot 'artifacts\acceptance'
if ([string]::IsNullOrWhiteSpace($OutputPath)) {
    $OutputPath = Join-Path $artifactDir 'task-delegation-graph-manifest.json'
}
$manifestPath = [IO.Path]::GetFullPath($OutputPath)
$manifestDirectory = Split-Path -Parent $manifestPath
$script:runId = [Guid]::NewGuid().ToString('N')
$tempBase = [IO.Path]::GetFullPath([IO.Path]::GetTempPath())
$script:runRoot = [IO.Path]::GetFullPath((Join-Path $tempBase "lazoo-task-graph-$script:runId"))
if (-not $script:runRoot.StartsWith($tempBase, [StringComparison]::OrdinalIgnoreCase)) {
    throw 'acceptance run root is outside the system temporary directory'
}
$script:dataRoot = Join-Path $script:runRoot 'data'
$script:fixtureDir = Join-Path $script:runRoot 'fixture-bin'
$script:fixtureLog = Join-Path $script:runRoot 'fixture.log'
$script:commands = [Collections.Generic.List[object]]::new()
$script:createdProcesses = [Collections.Generic.List[Diagnostics.Process]]::new()
$startedAt = [DateTime]::UtcNow
$stopwatch = [Diagnostics.Stopwatch]::StartNew()
$daemon = $null
$manifestStatus = 'FAIL'
$gates = [ordered]@{}
$evidence = [ordered]@{
    rootIdentity = $false
    replayNoDuplicate = $false
    followUpJoinedRoot = $false
    askProposalNoMessage = $false
    userAcceptanceCanonicalMessage = $false
    authorityNarrowed = $false
    guardFailures = $false
    nativeEvidenceRequired = $false
    evidenceOrdered = $false
    activeDescendantBlocked = $false
    childResultNoticeCompletesParent = $false
    ownerLossUncertain = $false
    uncertaintyResolutionIdempotent = $false
    restartCorrelations = $false
    providerSmokeAbsent = $false
    externalModelUseAbsent = $false
    worktreeMutationAbsent = $false
    conptyAbsent = $false
    desktopCapabilityExpansionAbsent = $false
    uiChangesAbsent = $false
}

New-Item -ItemType Directory -Force -Path $artifactDir, $manifestDirectory, $script:dataRoot | Out-Null

try {
    if (-not $SkipBuild) {
        $timer = [Diagnostics.Stopwatch]::StartNew()
        & $cargoExe build --workspace --release --locked
        $buildExit = $LASTEXITCODE
        $timer.Stop()
        Add-Command 'cargo build --workspace --release --locked' $buildExit $timer.ElapsedMilliseconds
        Assert-Equal $buildExit 0 'locked release build'
    }
    foreach ($binary in @($script:daemonExe, $script:ctlExe, $fixtureExe)) {
        Assert-True (Test-Path -LiteralPath $binary) "binary exists: $binary"
    }
    New-Item -ItemType Directory -Force -Path $script:fixtureDir | Out-Null
    Copy-Item -LiteralPath $fixtureExe -Destination (Join-Path $script:fixtureDir 'codex.exe')
    $gates.releaseBuild = if ($SkipBuild) { 'SKIPPED_BY_CALLER' } else { 'PASS' }

    $daemon = Start-Daemon
    $status = Wait-Status
    Assert-Equal $status.schemaVersion 13 'schema 13'
    Assert-Equal $status.journalMode 'wal' 'SQLite journal mode'
    $gates.schema8 = 'PASS'

    $lead = Invoke-Json @(
        'employee', 'create', 'Rhea', '--provider', 'codex', '--role', 'Lead',
        '--authority-mode', 'full-autonomy', '--idempotency-key', "lead-$script:runId"
    )
    $worker = Invoke-Json @(
        'employee', 'create', 'Mira', '--provider', 'codex', '--role', 'Worker',
        '--authority-mode', 'self-decision', '--idempotency-key', "worker-$script:runId"
    )
    $staleWorker = Invoke-Json @(
        'employee', 'create', 'Ivo', '--provider', 'codex', '--role', 'Worker',
        '--authority-mode', 'self-decision', '--idempotency-key', "stale-worker-$script:runId"
    )
    $disabledWorker = Invoke-Json @(
        'employee', 'create', 'Nia', '--provider', 'codex', '--role', 'Worker', '--disabled',
        '--authority-mode', 'self-decision', '--idempotency-key', "disabled-worker-$script:runId"
    )
    $leadId = [string]$lead.employee.id
    $workerId = [string]$worker.employee.id
    $staleWorkerId = [string]$staleWorker.employee.id
    $disabledWorkerId = [string]$disabledWorker.employee.id

    $conversation = Invoke-Json @(
        'conversation', 'create', $leadId, '--title', 'Task graph acceptance',
        '--idempotency-key', "conversation-$script:runId"
    )
    $conversationId = [string]$conversation.conversation.id
    $rootObjectives = @(
        'ROOT_FOLLOWUP',
        'ROOT_ASK_COMPLETE_USER',
        'ROOT_AUTHORITY',
        'ROOT_GUARDS',
        'ROOT_RESTART_HOLD_FOR_RESTART'
    )
    foreach ($objective in $rootObjectives) {
        $authorityArguments = if ($objective -eq 'ROOT_ASK_COMPLETE_USER') { @() } else { @('--authority-mode', 'full-autonomy') }
        Invoke-ExpectedFailure 12 'LZ-TURN-SESSION-NOT-READY' (
            @('turn', 'submit', $conversationId, '--text', $objective) + $authorityArguments +
            @('--idempotency-key', "root-$objective-$script:runId")
        ) | Out-Null
    }

    $roots = @(Invoke-Json @('task', 'list', '--conversation-id', $conversationId, '--limit', '100'))
    Assert-Equal $roots.Count $rootObjectives.Count 'root task count'
    $rootFollow = Get-TaskByObjective $roots 'ROOT_FOLLOWUP'
    $rootAsk = Get-TaskByObjective $roots 'ROOT_ASK_COMPLETE_USER'
    $rootAuthority = Get-TaskByObjective $roots 'ROOT_AUTHORITY'
    $rootGuards = Get-TaskByObjective $roots 'ROOT_GUARDS'
    $rootRestart = Get-TaskByObjective $roots 'ROOT_RESTART_HOLD_FOR_RESTART'

    $rootDetail = Invoke-Json @('task', 'show', ([string]$rootFollow.id))
    Assert-Equal $rootDetail.task.id $rootDetail.task.creationTurnId 'root task and creation turn identity'
    Assert-Equal $rootDetail.task.rootTaskId $rootDetail.task.id 'root ancestry identity'
    $turns = @(Invoke-Json @('turn', 'list', $conversationId, '--after-sequence', '0', '--limit', '100'))
    Assert-True (@($turns | Where-Object { [string]$_.id -eq [string]$rootFollow.id }).Count -eq 1) 'root turn exists once'
    $evidence.rootIdentity = $true

    $taskCountBeforeReplay = $roots.Count
    $evidenceCountBeforeReplay = @($rootDetail.evidence).Count
    Invoke-ExpectedFailure 12 'LZ-TURN-SESSION-NOT-READY' @(
        'turn', 'submit', $conversationId, '--text', 'ROOT_FOLLOWUP',
        '--authority-mode', 'full-autonomy',
        '--idempotency-key', "root-ROOT_FOLLOWUP-$script:runId"
    ) | Out-Null
    $rootsAfterReplay = @(Invoke-Json @('task', 'list', '--conversation-id', $conversationId, '--limit', '100'))
    $replayedDetail = Invoke-Json @('task', 'show', ([string]$rootFollow.id))
    Assert-Equal $rootsAfterReplay.Count $taskCountBeforeReplay 'replay task count'
    Assert-Equal @($replayedDetail.evidence).Count $evidenceCountBeforeReplay 'replay evidence count'
    $evidence.replayNoDuplicate = $true

    $leadWarm = Invoke-Json @(
        'session', 'warm', $leadId, '--expected-employee-revision', ([string]$lead.employee.configurationRevision),
        '--idempotency-key', "lead-warm-$script:runId"
    )
    $workerWarm = Invoke-Json @(
        'session', 'warm', $workerId, '--expected-employee-revision', ([string]$worker.employee.configurationRevision),
        '--idempotency-key', "worker-warm-$script:runId"
    )
    $leadSessionId = [string]$leadWarm.session.id
    $workerSessionId = [string]$workerWarm.session.id

    $roster = Invoke-Json @('roster', 'show')
    $rosterRevision = [long]$roster.revision
    $messagesBeforeProposal = @(Invoke-Json @('message', 'list', '--after-sequence', '0', '--limit', '1024')).Count
    $proposed = Invoke-Delegate ([string]$rootAsk.id) $leadId $leadSessionId $workerId `
        'ASK_CHILD' 'ask' $rosterRevision "ask-child-$script:runId"
    Assert-Equal $proposed.task.state 'proposed' 'ask child state'
    Assert-True ($null -eq $proposed.delegationMessage) 'ask child has no delegation message'
    $messagesAfterProposal = @(Invoke-Json @('message', 'list', '--after-sequence', '0', '--limit', '1024')).Count
    Assert-Equal $messagesAfterProposal $messagesBeforeProposal 'ask proposal message count'
    Invoke-ExpectedFailure 11 'LZ-MESSAGE-NOT-FOUND' @(
        'message', 'claim', ([string]$proposed.task.id), '--expected-revision', '1',
        '--idempotency-key', "claim-proposal-$script:runId"
    ) | Out-Null
    $evidence.askProposalNoMessage = $true

    $accepted = Invoke-Json @(
        'task', 'accept', ([string]$proposed.task.id), '--expected-revision', ([string]$proposed.task.revision),
        '--idempotency-key', "accept-proposal-$script:runId"
    )
    Assert-Equal $accepted.task.state 'accepted' 'accepted child state'
    Assert-Equal $accepted.delegationMessage.envelope.kind 'delegation' 'canonical delegation kind'
    Assert-Equal $accepted.delegationMessage.envelope.taskId $accepted.task.id 'canonical delegation task link'
    Assert-Equal $accepted.delegationMessage.envelope.sender.employeeId $leadId 'canonical delegation sender'
    Assert-Equal $accepted.delegationMessage.envelope.recipient.employeeId $workerId 'canonical delegation recipient'
    $acceptedDetail = Invoke-Json @('task', 'show', ([string]$accepted.task.id))
    Assert-Equal @($acceptedDetail.evidence | Where-Object { $_.kind -eq 'accepted' }).Count 1 'single accepted evidence'
    $evidence.userAcceptanceCanonicalMessage = $true

    Invoke-ExpectedFailure 13 'LZ-TASK-ACCEPTANCE-EVIDENCE' @(
        'task', 'start', ([string]$accepted.task.id), '--expected-revision', ([string]$accepted.task.revision),
        '--actor-employee', $workerId, '--actor-session', $workerSessionId,
        '--turn-id', ([string]$rootAsk.id),
        '--idempotency-key', "invalid-start-$script:runId"
    ) | Out-Null
    $evidence.nativeEvidenceRequired = $true

    $childCommand = Start-JsonCommand 'active-child' @(
        'turn', 'submit', $conversationId, '--recipient-employee', $workerId,
        '--task-id', ([string]$accepted.task.id), '--authority-mode', 'ask',
        '--text', 'HOLD_FOR_RESTART', '--idempotency-key', "active-child-turn-$script:runId"
    )
    $activeChild = Wait-TaskState ([string]$accepted.task.id) @('running')
    $parentTurn = Invoke-Json @(
        'turn', 'submit', $conversationId, '--text', 'ROOT_ASK_COMPLETE_USER',
        '--idempotency-key', "root-ROOT_ASK_COMPLETE_USER-$script:runId"
    )
    Assert-Equal $parentTurn.turn.id $rootAsk.id 'parent creation turn replay identity'
    Assert-Equal $parentTurn.turn.state 'completed' 'parent native turn completion'
    $taskCountBeforeFollowUp = @(Invoke-Json @(
        'task', 'list', '--conversation-id', $conversationId, '--limit', '100'
    )).Count
    $followUp = Invoke-Json @(
        'turn', 'submit', $conversationId, '--task-id', ([string]$rootAsk.id),
        '--authority-mode', 'ask', '--text', 'COMPLETE_USER',
        '--idempotency-key', "parent-follow-up-$script:runId"
    )
    Assert-True ([string]$followUp.turn.id -ne [string]$rootAsk.id) 'follow-up turn identity'
    Assert-Equal $followUp.turn.taskId $rootAsk.id 'follow-up root task identity'
    Assert-Equal $followUp.task.id $rootAsk.id 'follow-up result task identity'
    $tasksAfterFollowUp = @(Invoke-Json @(
        'task', 'list', '--conversation-id', $conversationId, '--limit', '100'
    ))
    Assert-Equal $tasksAfterFollowUp.Count $taskCountBeforeFollowUp 'follow-up task count'
    $evidence.followUpJoinedRoot = $true
    $activeParent = Invoke-Json @('task', 'show', ([string]$rootAsk.id))
    Assert-Equal $activeParent.task.state 'running' 'parent and child run concurrently'
    Assert-True ($null -eq $activeParent.task.activeTurnId) 'completed parent turn releases execution ownership'
    Invoke-ExpectedFailure 13 'LZ-TASK-CHILDREN-ACTIVE' @(
        'task', 'cancel', ([string]$rootAsk.id), '--expected-revision', ([string]$activeParent.task.revision),
        '--idempotency-key', "blocked-parent-cancel-$script:runId"
    ) | Out-Null
    Invoke-ExpectedFailure 13 'LZ-TASK-CHILDREN-ACTIVE' @(
        'task', 'report', ([string]$rootAsk.id), '--expected-revision', ([string]$activeParent.task.revision),
        '--actor-employee', $leadId, '--actor-session', $leadSessionId, '--kind', 'completed',
        '--idempotency-key', "blocked-parent-complete-$script:runId"
    ) | Out-Null
    $evidence.activeDescendantBlocked = $true

    $childTurn = Invoke-Json @('turn', 'show', ([string]$activeChild.task.activeTurnId))
    Invoke-Json @(
        'turn', 'cancel', ([string]$childTurn.id), '--expected-revision', ([string]$childTurn.revision),
        '--idempotency-key', "cancel-active-child-$script:runId"
    ) | Out-Null
    $childSubmit = Complete-JsonCommand $childCommand
    Assert-Equal $childSubmit.turn.state 'cancelled' 'child native cancellation'
    $cancelledChild = Wait-TaskState ([string]$accepted.task.id) @('cancelled')
    $childEvidence = @($cancelledChild.evidence)
    Assert-True ($childEvidence.Count -ge 4) 'child evidence count'
    $evidenceSequences = @($childEvidence | ForEach-Object { [long]$_.sequence })
    for ($index = 1; $index -lt $evidenceSequences.Count; $index++) {
        Assert-True ($evidenceSequences[$index] -gt $evidenceSequences[$index - 1]) 'evidence sequence order'
    }
    $evidenceKinds = @($childEvidence | ForEach-Object { [string]$_.kind })
    foreach ($kind in @('proposed', 'accepted', 'started', 'progress', 'cancelled')) {
        Assert-True ($evidenceKinds -contains $kind) "child evidence kind $kind"
    }
    $evidence.evidenceOrdered = $true
    # The cancelled child's outcome reaches the lead as a result notice on its idle session. The
    # lead's notice turn ends with no unfinished child, which completes the parent, so the lead's
    # own later completion report is refused as already terminal.
    $noticedParent = Wait-TaskState ([string]$rootAsk.id) @('completed')
    $resultNotices = @(Invoke-Json @('message', 'list', '--after-sequence', '0', '--limit', '1024') |
        Where-Object { [string]$_.kind -eq 'result' -and [string]$_.taskId -eq [string]$rootAsk.id })
    Assert-Equal $resultNotices.Count 1 'one result notice per terminal child'
    Assert-Equal ([string]$resultNotices[0].state) 'acknowledged' 'result notice delivered to the lead'
    Invoke-ExpectedFailure 13 'LZ-TASK-STATE' @(
        'task', 'report', ([string]$rootAsk.id), '--expected-revision', ([string]$noticedParent.task.revision),
        '--actor-employee', $leadId, '--actor-session', $leadSessionId, '--kind', 'completed',
        '--idempotency-key', "complete-parent-$script:runId"
    ) | Out-Null
    $evidence.childResultNoticeCompletesParent = $true

    $bounded = Invoke-Delegate ([string]$rootAuthority.id) $leadId $leadSessionId $workerId `
        'DEPTH_1' 'self-decision' $rosterRevision "depth-1-$script:runId"
    Assert-Equal $bounded.task.state 'accepted' 'non-ask accepted state'
    Assert-Equal $bounded.task.requestedAuthority 'self-decision' 'bounded requested authority'
    Assert-Equal $bounded.task.effectiveAuthority 'self-decision' 'bounded effective authority'
    Assert-True ($null -ne $bounded.delegationMessage) 'non-ask canonical message'
    $evidence.authorityNarrowed = $true

    Invoke-ExpectedFailure 13 'LZ-TASK-ROSTER-STALE' @(
        'task', 'delegate', ([string]$rootGuards.id), '--actor-employee', $leadId,
        '--actor-session', $leadSessionId, '--assigned-employee', $workerId,
        '--objective', 'STALE_ROSTER', '--requested-authority', 'self-decision',
        '--roster-revision', ([string]($rosterRevision - 1)),
        '--idempotency-key', "stale-roster-$script:runId"
    ) | Out-Null
    Invoke-ExpectedFailure 13 'LZ-TASK-SELF-DELEGATION' @(
        'task', 'delegate', ([string]$rootGuards.id), '--actor-employee', $leadId,
        '--actor-session', $leadSessionId, '--assigned-employee', $leadId,
        '--objective', 'SELF', '--requested-authority', 'self-decision',
        '--roster-revision', ([string]$rosterRevision),
        '--idempotency-key', "self-$script:runId"
    ) | Out-Null
    Invoke-ExpectedFailure 13 'LZ-TASK-ASSIGNEE-UNAVAILABLE' @(
        'task', 'delegate', ([string]$rootGuards.id), '--actor-employee', $leadId,
        '--actor-session', $leadSessionId, '--assigned-employee', $disabledWorkerId,
        '--objective', 'UNAVAILABLE', '--requested-authority', 'self-decision',
        '--roster-revision', ([string]$rosterRevision),
        '--idempotency-key', "unavailable-$script:runId"
    ) | Out-Null

    $staleProposal = Invoke-Delegate ([string]$rootGuards.id) $leadId $leadSessionId $staleWorkerId `
        'STALE_CONFIGURATION' 'ask' $rosterRevision "stale-config-task-$script:runId"
    Assert-Equal $staleProposal.task.state 'proposed' 'stale configuration fixture stays undispatched'
    $updatedStaleWorker = Invoke-Json @(
        'employee', 'update', $staleWorkerId, '--expected-revision', ([string]$staleWorker.employee.configurationRevision),
        '--role', 'Updated worker', '--idempotency-key', "update-stale-worker-$script:runId"
    )
    $staleWarm = Invoke-Json @(
        'session', 'warm', $staleWorkerId,
        '--expected-employee-revision', ([string]$updatedStaleWorker.employee.configurationRevision),
        '--idempotency-key', "warm-stale-worker-$script:runId"
    )
    $staleTask = Invoke-Json @(
        'task', 'accept', ([string]$staleProposal.task.id),
        '--expected-revision', ([string]$staleProposal.task.revision),
        '--idempotency-key', "accept-stale-worker-$script:runId"
    )
    $staleCommand = Start-JsonCommand 'stale-config-turn' @(
        'turn', 'submit', $conversationId, '--recipient-employee', $staleWorkerId,
        '--task-id', ([string]$staleTask.task.id), '--authority-mode', 'self-decision',
        '--text', 'HOLD_FOR_RESTART', '--idempotency-key', "stale-config-turn-$script:runId"
    )
    $runningStale = Wait-TaskState ([string]$staleTask.task.id) @('running')
    Invoke-ExpectedFailure 13 'LZ-TASK-EMPLOYEE-STALE' @(
        'task', 'report', ([string]$staleTask.task.id), '--expected-revision', ([string]$runningStale.task.revision),
        '--actor-employee', $staleWorkerId, '--actor-session', ([string]$staleWarm.session.id),
        '--kind', 'progress',
        '--idempotency-key', "stale-config-report-$script:runId"
    ) | Out-Null
    $staleTurn = Invoke-Json @('turn', 'show', ([string]$runningStale.task.activeTurnId))
    Invoke-Json @(
        'turn', 'cancel', ([string]$staleTurn.id), '--expected-revision', ([string]$staleTurn.revision),
        '--idempotency-key', "cancel-stale-turn-$script:runId"
    ) | Out-Null
    Complete-JsonCommand $staleCommand | Out-Null
    Wait-TaskState ([string]$staleTask.task.id) @('cancelled') | Out-Null

    $currentRoster = Invoke-Json @('roster', 'show')
    $currentRosterRevision = [long]$currentRoster.revision
    $depth2 = Invoke-Delegate ([string]$bounded.task.id) $workerId $workerSessionId $leadId `
        'DEPTH_2' 'self-decision' $currentRosterRevision "depth-2-$script:runId"
    $depth3 = Invoke-Delegate ([string]$depth2.task.id) $leadId $leadSessionId $workerId `
        'DEPTH_3' 'self-decision' $currentRosterRevision "depth-3-$script:runId"
    $depth4 = Invoke-Delegate ([string]$depth3.task.id) $workerId $workerSessionId $leadId `
        'DEPTH_4' 'self-decision' $currentRosterRevision "depth-4-$script:runId"
    Assert-Equal $depth4.task.depth 4 'maximum accepted depth'
    Invoke-ExpectedFailure 13 'LZ-TASK-DEPTH-LIMIT' @(
        'task', 'delegate', ([string]$depth4.task.id), '--actor-employee', $leadId,
        '--actor-session', $leadSessionId, '--assigned-employee', $workerId,
        '--objective', 'DEPTH_5', '--requested-authority', 'self-decision',
        '--roster-revision', ([string]$currentRosterRevision),
        '--idempotency-key', "depth-5-$script:runId"
    ) | Out-Null

    for ($fanout = 1; $fanout -le 8; $fanout++) {
        Invoke-Delegate ([string]$rootGuards.id) $leadId $leadSessionId $workerId `
            "FANOUT_$fanout" 'self-decision' $currentRosterRevision "fanout-$fanout-$script:runId" | Out-Null
    }
    Invoke-ExpectedFailure 13 'LZ-TASK-FANOUT-LIMIT' @(
        'task', 'delegate', ([string]$rootGuards.id), '--actor-employee', $leadId,
        '--actor-session', $leadSessionId, '--assigned-employee', $workerId,
        '--objective', 'FANOUT_9', '--requested-authority', 'self-decision',
        '--roster-revision', ([string]$currentRosterRevision),
        '--idempotency-key', "fanout-9-$script:runId"
    ) | Out-Null
    $evidence.guardFailures = $true

    $restartCommand = Start-JsonCommand 'restart-turn' @(
        'turn', 'submit', $conversationId, '--authority-mode', 'full-autonomy',
        '--text', 'ROOT_RESTART_HOLD_FOR_RESTART',
        '--idempotency-key', "root-ROOT_RESTART_HOLD_FOR_RESTART-$script:runId"
    )
    $runningRestart = Wait-TaskState ([string]$rootRestart.id) @('running')
    $restartTurnId = [string]$runningRestart.task.activeTurnId
    $restartSessionId = [string]$runningRestart.task.activeSessionId
    $priorBootId = [string]$status.bootId
    $daemon.Kill()
    $daemon.WaitForExit(10000) | Out-Null
    $daemon = $null
    Complete-InterruptedCommand $restartCommand
    $daemon = Start-Daemon
    $status = Wait-Status $priorBootId
    $uncertain = Wait-TaskState ([string]$rootRestart.id) @('uncertain')
    Assert-Equal $uncertain.task.failureCode 'LZ-TASK-OWNER-LOST' 'task owner loss code'
    Assert-True ($null -eq $uncertain.task.activeTurnId) 'uncertain task clears active turn'
    Assert-True ($null -eq $uncertain.task.activeSessionId) 'uncertain task clears active session'
    Assert-True (@($uncertain.evidence | Where-Object { $_.kind -eq 'uncertain' }).Count -eq 1) 'single uncertainty evidence'
    $evidence.ownerLossUncertain = $true

    $resolved = Invoke-Json @(
        'task', 'resolve', ([string]$rootRestart.id), '--expected-revision', ([string]$uncertain.task.revision),
        '--resolution', 'completed',
        '--idempotency-key', "resolve-restart-$script:runId"
    )
    $resolvedDetail = Invoke-Json @('task', 'show', ([string]$rootRestart.id))
    $resolvedEvidenceCount = @($resolvedDetail.evidence).Count
    $resolveReplay = Invoke-Json @(
        'task', 'resolve', ([string]$rootRestart.id), '--expected-revision', ([string]$uncertain.task.revision),
        '--resolution', 'completed',
        '--idempotency-key', "resolve-restart-$script:runId"
    )
    $replayedResolvedDetail = Invoke-Json @('task', 'show', ([string]$rootRestart.id))
    Assert-Equal $resolved.task.state 'completed' 'uncertainty resolution state'
    Assert-Equal $resolveReplay.task.id $resolved.task.id 'uncertainty resolution replay identity'
    Assert-Equal @($replayedResolvedDetail.evidence).Count $resolvedEvidenceCount 'uncertainty replay evidence count'
    $evidence.uncertaintyResolutionIdempotent = $true

    $events = @(Invoke-Json @('events', 'list', '--after', '0', '--limit', '1024'))
    $eventTypes = @($events | ForEach-Object { [string]$_.eventType })
    foreach ($requiredEvent in @(
        'task.created', 'task.delegated', 'task.accepted', 'task.started',
        'task.progressed', 'task.completed', 'task.cancelled', 'tasks.reconciled'
    )) {
        Assert-True ($eventTypes -contains $requiredEvent) "audit/outbox event $requiredEvent"
    }
    $reconcileEvent = @($events | Where-Object { $_.eventType -eq 'tasks.reconciled' })[-1]
    Assert-True ($null -ne $reconcileEvent) 'task reconciliation event'
    Assert-Equal $uncertain.task.id $rootRestart.id 'restart task identity'
    Assert-True (-not [string]::IsNullOrWhiteSpace($restartTurnId)) 'restart turn correlation'
    Assert-True (-not [string]::IsNullOrWhiteSpace($restartSessionId)) 'restart session correlation'
    $drained = Wait-Status
    Assert-Equal $drained.pendingOutbox 0 'outbox drained'
    $evidence.restartCorrelations = $true

    $commandTexts = @($script:commands | ForEach-Object { [string]$_.command })
    Assert-True (@($commandTexts | Where-Object { $_ -match 'provider\s+smoke' }).Count -eq 0) 'provider smoke absent'
    Assert-True (@($commandTexts | Where-Object { $_ -match 'worktree|conpty' }).Count -eq 0) 'deferred execution controls absent'
    $fixtureLines = @(Get-Content -LiteralPath $script:fixtureLog)
    Assert-True ($fixtureLines -contains 'turn/start') 'deterministic fixture used for native evidence'
    $evidence.providerSmokeAbsent = $true
    $evidence.externalModelUseAbsent = $true
    $evidence.worktreeMutationAbsent = $true
    $evidence.conptyAbsent = $true

    $mergeBase = (& git -C $repoRoot merge-base HEAD main).Trim()
    Assert-Equal $LASTEXITCODE 0 'git merge base'
    $desktopDiff = @(& git -C $repoRoot diff --name-only $mergeBase -- apps/lazoo-desktop)
    Assert-Equal $LASTEXITCODE 0 'desktop committed diff check'
    $desktopStatus = @(& git -C $repoRoot status --porcelain -- apps/lazoo-desktop)
    Assert-Equal $LASTEXITCODE 0 'desktop worktree status check'
    Assert-Equal $desktopDiff.Count 0 'desktop committed changes absent'
    Assert-Equal $desktopStatus.Count 0 'desktop worktree changes absent'
    $evidence.desktopCapabilityExpansionAbsent = $true
    $evidence.uiChangesAbsent = $true

    Stop-Daemon $daemon
    $daemon = $null
    $manifestStatus = 'PASS'
    $gates.taskGraph = 'PASS'
} catch {
    $gates.failure = $_.Exception.Message
    throw
} finally {
    $stopwatch.Stop()
    if ($null -ne $daemon -and -not $daemon.HasExited) {
        try { $daemon.Kill() } catch [InvalidOperationException] {}
        $daemon.WaitForExit(5000) | Out-Null
    }
    foreach ($created in $script:createdProcesses) {
        try {
            $created.Refresh()
            if (-not $created.HasExited) {
                $created.Kill()
                $created.WaitForExit(5000) | Out-Null
            }
        } catch [InvalidOperationException] {}
    }
    $commandSnapshot = @($script:commands | ForEach-Object {
        [ordered]@{
            command = [string]$_.command
            exitCode = [int]$_.exitCode
            durationMs = if ($null -eq $_.durationMs) { $null } else { [long]$_.durationMs }
        }
    })
    $manifest = [ordered]@{
        schemaVersion = 13
        acceptance = 'task-delegation-graph'
        mode = 'deterministic-fixture'
        status = $manifestStatus
        startedAt = $startedAt.ToString('o')
        finishedAt = [DateTime]::UtcNow.ToString('o')
        durationMs = $stopwatch.ElapsedMilliseconds
        gates = $gates
        evidence = $evidence
        commands = $commandSnapshot
    }
    $manifestJson = $manifest | ConvertTo-Json -Depth 24
    [IO.File]::WriteAllText($manifestPath, $manifestJson, [Text.UTF8Encoding]::new($false))
    if ($script:runRoot.StartsWith($tempBase, [StringComparison]::OrdinalIgnoreCase) -and
        (Test-Path -LiteralPath $script:runRoot)) {
        Remove-Item -LiteralPath $script:runRoot -Recurse -Force
    }
}

Write-Output "PASS manifest: $manifestPath"
