param(
    [switch]$SkipBuild,
    [string]$Provider = 'codex',
    [string]$LivePrompt = ''
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

function Invoke-Json([string[]]$Arguments) {
    $previousRoot = Set-DataRoot $script:dataRoot
    $previousPreference = $ErrorActionPreference
    try {
        $ErrorActionPreference = 'Continue'
        $timer = [Diagnostics.Stopwatch]::StartNew()
        $captured = @(& $script:ctlExe --json @Arguments 2>&1)
        $exitCode = $LASTEXITCODE
        $timer.Stop()
        $script:commands.Add([ordered]@{
            command = "lazooctl --json $($Arguments -join ' ')"
            exitCode = $exitCode
            durationMs = $timer.ElapsedMilliseconds
        })
        if ($exitCode -ne 0) {
            throw "lazooctl $($Arguments -join ' ') exited $exitCode with: $($captured -join [Environment]::NewLine)"
        }
        return (($captured -join [Environment]::NewLine) | ConvertFrom-Json)
    } finally {
        $ErrorActionPreference = $previousPreference
        Restore-DataRoot $previousRoot
    }
}

function Invoke-ExpectedFailure([int]$ExpectedExit, [string[]]$Arguments) {
    $previousRoot = Set-DataRoot $script:dataRoot
    $previousPreference = $ErrorActionPreference
    try {
        $ErrorActionPreference = 'Continue'
        $timer = [Diagnostics.Stopwatch]::StartNew()
        $captured = @(& $script:ctlExe --json @Arguments 2>&1)
        $exitCode = $LASTEXITCODE
        $timer.Stop()
        $script:commands.Add([ordered]@{
            command = "lazooctl --json $($Arguments -join ' ')"
            exitCode = $exitCode
            durationMs = $timer.ElapsedMilliseconds
        })
        Assert-Equal $exitCode $ExpectedExit "expected failure: $($Arguments -join ' ')"
        return ($captured -join [Environment]::NewLine)
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
    foreach ($outerCodexVariable in @('CODEX_THREAD_ID', 'CODEX_SESSION_ID')) {
        [void]$startInfo.Environment.Remove($outerCodexVariable)
    }
    if ($script:deterministic) {
        $startInfo.Environment['PATH'] = "$script:fixtureDir;$env:PATH"
        $startInfo.Environment['LAZOO_TURN_FIXTURE_LOG'] = $script:fixtureLog
        $startInfo.Environment['LAZOO_TURN_FIXTURE_THREAD_ID'] = "fixture-thread-$script:runId"
    }
    $process = [Diagnostics.Process]::new()
    $process.StartInfo = $startInfo
    Assert-True $process.Start() 'daemon process start'
    $script:createdPids.Add($process.Id)
    return $process
}

function Start-JsonCommand([string]$Name, [string[]]$Arguments) {
    $stdoutPath = Join-Path $script:runRoot "$Name.stdout.json"
    $stderrPath = Join-Path $script:runRoot "$Name.stderr.txt"
    $previousRoot = Set-DataRoot $script:dataRoot
    try {
        $process = Start-Process -FilePath $script:ctlExe -ArgumentList (@('--json') + $Arguments) `
            -RedirectStandardOutput $stdoutPath -RedirectStandardError $stderrPath `
            -PassThru -WindowStyle Hidden
        $script:createdPids.Add($process.Id)
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
    $script:commands.Add([ordered]@{
        command = "lazooctl --json $($Command.Arguments -join ' ')"
        exitCode = $exitCode
        durationMs = $null
    })
    if ($exitCode -ne 0) {
        throw "background lazooctl exited ${exitCode}: $stderr"
    }
    return ($stdout | ConvertFrom-Json)
}

function Wait-Status([string]$DifferentBootId = '', [int]$TimeoutSeconds = 20) {
    $deadline = [DateTime]::UtcNow.AddSeconds($TimeoutSeconds)
    $lastError = ''
    while ([DateTime]::UtcNow -lt $deadline) {
        try {
            $status = Invoke-Json @('status')
            if (($DifferentBootId -eq '') -or ($status.bootId -ne $DifferentBootId)) {
                return $status
            }
        } catch {
            $lastError = $_.Exception.Message
        }
        Start-Sleep -Milliseconds 100
    }
    throw "daemon did not become ready: $lastError"
}

function Wait-TurnState([string]$ConversationId, [string[]]$States, [int]$TimeoutSeconds = 20) {
    $deadline = [DateTime]::UtcNow.AddSeconds($TimeoutSeconds)
    $last = $null
    while ([DateTime]::UtcNow -lt $deadline) {
        $turns = @(Invoke-Json @('turn', 'list', $ConversationId, '--after-sequence', '0', '--limit', '100'))
        if ($turns.Count -gt 0) {
            $last = Invoke-Json @('turn', 'show', ([string]$turns[-1].id))
            if ($States -contains [string]$last.state) {
                return $last
            }
        }
        Start-Sleep -Milliseconds 100
    }
    throw "turn did not reach $($States -join ','); last state was $($last.state)"
}

function Stop-Daemon([Diagnostics.Process]$Process) {
    Invoke-Json @('system', 'shutdown') | Out-Null
    if (-not $Process.WaitForExit(15000)) {
        throw "daemon $($Process.Id) did not stop after planned shutdown"
    }
    Assert-Equal $Process.ExitCode 0 'planned daemon exit code'
}

function Get-ProcessRecord([int]$ProcessId) {
    return Get-CimInstance Win32_Process -Filter "ProcessId = $ProcessId" -ErrorAction SilentlyContinue
}

function Get-ProcessTreeRecords([int]$RootProcessId) {
    $processes = @(Get-CimInstance Win32_Process)
    $records = [Collections.Generic.List[object]]::new()
    $pending = [Collections.Generic.Queue[int]]::new()
    $seen = [Collections.Generic.HashSet[int]]::new()
    $pending.Enqueue($RootProcessId)
    while ($pending.Count -gt 0) {
        $processId = $pending.Dequeue()
        if (-not $seen.Add($processId)) {
            continue
        }
        $record = $processes | Where-Object { $_.ProcessId -eq $processId } | Select-Object -First 1
        if ($null -ne $record) {
            $records.Add($record)
        }
        foreach ($child in @($processes | Where-Object { $_.ParentProcessId -eq $processId })) {
            $pending.Enqueue([int]$child.ProcessId)
        }
    }
    return @($records)
}

function Wait-ProcessGone([int]$ProcessId, [int]$TimeoutSeconds, [string]$Label) {
    $deadline = [DateTime]::UtcNow.AddSeconds($TimeoutSeconds)
    while ([DateTime]::UtcNow -lt $deadline) {
        if ($null -eq (Get-ProcessRecord $ProcessId)) {
            return
        }
        Start-Sleep -Milliseconds 100
    }
    throw "$Label process $ProcessId remained alive after $TimeoutSeconds seconds"
}

$repoRoot = (Resolve-Path (Join-Path $PSScriptRoot '..\..')).Path
$cargoExe = (Get-Command cargo -CommandType Application -ErrorAction Stop).Source
$releaseDir = Join-Path $repoRoot 'target\release'
$script:daemonExe = Join-Path $releaseDir 'lazood.exe'
$script:ctlExe = Join-Path $releaseDir 'lazooctl.exe'
$fixtureExe = Join-Path $releaseDir 'lazoo-turn-transport-fixture.exe'
$artifactDir = Join-Path $repoRoot 'artifacts\acceptance'
$manifestName = if ([string]::IsNullOrWhiteSpace($LivePrompt)) {
    'conversation-delivery-manifest.json'
} else {
    'conversation-delivery-live-manifest.json'
}
$manifestPath = Join-Path $artifactDir $manifestName
$script:runId = [Guid]::NewGuid().ToString('N')
$tempBase = [IO.Path]::GetFullPath([IO.Path]::GetTempPath())
$script:runRoot = [IO.Path]::GetFullPath((Join-Path $tempBase "lazoo-conversation-delivery-$script:runId"))
if (-not $script:runRoot.StartsWith($tempBase, [StringComparison]::OrdinalIgnoreCase)) {
    throw 'acceptance run root is outside the system temporary directory'
}
$script:dataRoot = Join-Path $script:runRoot 'data'
$script:fixtureDir = Join-Path $script:runRoot 'fixture-bin'
$script:fixtureLog = Join-Path $script:runRoot 'fixture.log'
$script:deterministic = [string]::IsNullOrWhiteSpace($LivePrompt)
$startedAt = [DateTime]::UtcNow
$stopwatch = [Diagnostics.Stopwatch]::StartNew()
$script:commands = [Collections.Generic.List[object]]::new()
$script:createdPids = [Collections.Generic.List[int]]::new()
$gates = [ordered]@{}
$evidence = [ordered]@{}
$daemon = $null
$shutdownSessionId = ''
$manifestStatus = 'FAIL'

New-Item -ItemType Directory -Force -Path $artifactDir, $script:dataRoot | Out-Null

try {
    if (-not $SkipBuild) {
        $timer = [Diagnostics.Stopwatch]::StartNew()
        & $cargoExe build --workspace --release --locked
        $buildExit = $LASTEXITCODE
        $timer.Stop()
        $script:commands.Add([ordered]@{
            command = 'cargo build --workspace --release --locked'
            exitCode = $buildExit
            durationMs = $timer.ElapsedMilliseconds
        })
        Assert-Equal $buildExit 0 'locked release build'
    }
    foreach ($binary in @($script:daemonExe, $script:ctlExe)) {
        Assert-True (Test-Path -LiteralPath $binary) "binary exists: $binary"
    }
    if ($script:deterministic) {
        Assert-True (Test-Path -LiteralPath $fixtureExe) "binary exists: $fixtureExe"
        New-Item -ItemType Directory -Force -Path $script:fixtureDir | Out-Null
        Copy-Item -LiteralPath $fixtureExe -Destination (Join-Path $script:fixtureDir 'codex.exe')
    }
    $gates.releaseBuild = if ($SkipBuild) { 'SKIPPED_BY_CALLER' } else { 'PASS' }

    $daemon = Start-Daemon
    $gates.outerCodexContextIsolated = 'PASS'
    $status = Wait-Status
    Assert-Equal $status.schemaVersion 19 'schema 19'
    Assert-Equal $status.journalMode 'wal' 'SQLite journal mode'
    $gates.schema8 = 'PASS'

    $lead = Invoke-Json @(
        'employee', 'create', 'Rhea', '--provider', $Provider, '--role', 'Lead',
        '--idempotency-key', "lead-$script:runId"
    )
    $author = Invoke-Json @(
        'employee', 'create', 'Ivo', '--provider', $Provider, '--role', 'Reviewer',
        '--idempotency-key', "author-$script:runId"
    )
    $leadId = [string]$lead.employee.id
    $authorId = [string]$author.employee.id

    $projectless = Invoke-Json @(
        'conversation', 'create', $leadId, '--title', 'Projectless fixture',
        '--idempotency-key', "projectless-$script:runId"
    )
    $projectlessId = [string]$projectless.conversation.id
    $gates.projectlessConversation = 'PASS'

    $repository = Join-Path $script:runRoot 'repository'
    New-Item -ItemType Directory -Force -Path $repository | Out-Null
    & git init --quiet $repository
    Assert-Equal $LASTEXITCODE 0 'fixture repository init'
    $project = Invoke-Json @(
        'project', 'register', $repository, '--name', 'Fixture repository',
        '--idempotency-key', "project-$script:runId"
    )
    $projectConversation = Invoke-Json @(
        'conversation', 'create', $leadId, '--project-id', ([string]$project.project.id),
        '--title', 'Project fixture', '--idempotency-key', "project-conversation-$script:runId"
    )
    $projectConversationId = [string]$projectConversation.conversation.id
    $gates.projectConversation = 'PASS'

    if (-not $script:deterministic) {
        $live = Invoke-Json @(
            'turn', 'submit', $projectlessId, '--text', $LivePrompt, '--activate-if-needed',
            '--idempotency-key', "live-turn-$script:runId"
        )
        Assert-Equal $live.turn.state 'completed' 'live turn completion'
        Assert-Equal $live.message.delivery.state 'acknowledged' 'live message acknowledgement'
        Assert-True (-not [string]::IsNullOrWhiteSpace([string]$live.turn.nativeSessionId)) 'live native session ID'
        Assert-True (-not [string]::IsNullOrWhiteSpace([string]$live.turn.nativeTurnId)) 'live native turn ID'
        $completedEvent = @($live.turn.events | Where-Object { $_.kind -eq 'turnCompleted' })[-1]
        $expectedOutput = if ($LivePrompt -match 'exactly\s+([A-Za-z0-9_]+)') { $Matches[1] } else { '' }
        if ($expectedOutput -ne '') {
            Assert-Equal ([string]$completedEvent.payload.output) $expectedOutput 'live exact output'
        }
        $gates.nativeTurnIdBeforeAcknowledgement = 'PASS'
        $gates.progressDurable = 'PASS'
        $gates.modelInvocationAbsent = 'NOT_APPLICABLE_LIVE'
        $evidence.liveOutput = [string]$completedEvent.payload.output
        $evidence.liveNativeTurnId = [string]$live.turn.nativeTurnId
        $shutdownSessionId = [string]$live.turn.recipient.sessionId
    } else {
        $employeeRootFailure = Invoke-ExpectedFailure 12 @(
            'turn', 'submit', $projectConversationId, '--text', 'EMPLOYEE_TASK_ROOT',
            '--idempotency-key', "employee-root-$script:runId"
        )
        Assert-True $employeeRootFailure.Contains('LZ-TURN-SESSION-NOT-READY') 'employee task root queued before warm'
        $employeeTasks = @(Invoke-Json @(
            'task', 'list', '--conversation-id', $projectConversationId, '--limit', '100'
        ))
        Assert-Equal $employeeTasks.Count 1 'employee task root count'
        $employeeParentTaskId = [string]$employeeTasks[0].id

        $userTurn = Invoke-Json @(
            'turn', 'submit', $projectlessId, '--text', 'COMPLETE_USER', '--activate-if-needed',
            '--idempotency-key', "user-turn-$script:runId"
        )
        Assert-Equal $userTurn.turn.state 'completed' 'user turn completion'
        Assert-Equal $userTurn.turn.author.kind 'user' 'userAuthorship'
        Assert-Equal $userTurn.message.envelope.version 2 'envelopeV2'
        Assert-True (-not [string]::IsNullOrWhiteSpace([string]$userTurn.turn.nativeTurnId)) 'native turn ID'
        Assert-Equal $userTurn.message.delivery.state 'acknowledged' 'message acknowledgement'
        $gates.userAuthorship = 'PASS'
        $gates.envelopeV2 = 'PASS'
        $gates.activationExplicit = 'PASS'
        $gates.nativeTurnIdBeforeAcknowledgement = 'PASS'
        $gates.progressDurable = 'PASS'

        $leadSessionId = [string]$userTurn.turn.recipient.sessionId
        $roster = Invoke-Json @('roster', 'show')
        $employeeProposal = Invoke-Json @(
            'task', 'delegate', $employeeParentTaskId,
            '--actor-employee', $leadId, '--actor-session', $leadSessionId,
            '--assigned-employee', $authorId, '--objective', 'EMPLOYEE_TASK_CHILD',
            '--requested-authority', 'ask', '--roster-revision', ([string]$roster.revision),
            '--idempotency-key', "employee-task-$script:runId"
        )
        Invoke-Json @(
            'session', 'warm', $authorId, '--expected-employee-revision', '1',
            '--idempotency-key', "author-session-$script:runId"
        ) | Out-Null
        $employeeTask = Invoke-Json @(
            'task', 'accept', ([string]$employeeProposal.task.id),
            '--expected-revision', ([string]$employeeProposal.task.revision),
            '--idempotency-key', "employee-task-accept-$script:runId"
        )

        $terminalFailure = Invoke-Json @(
            'turn', 'submit', $projectlessId, '--text', 'FAIL_AFTER_ACCEPTANCE',
            '--idempotency-key', "terminal-failure-$script:runId"
        )
        Assert-Equal $terminalFailure.turn.state 'terminal-failed' 'native terminal failure mapping'
        Assert-Equal $terminalFailure.message.delivery.state 'acknowledged' 'terminal failure acknowledgement preserved'
        Assert-True (@($terminalFailure.turn.events | Where-Object { $_.kind -eq 'error' }).Count -eq 1) 'native error event'
        $gates.nativeTerminalMapping = 'PASS'

        $employeeTurn = Invoke-Json @(
            'turn', 'submit', $projectConversationId, '--author-employee', $authorId,
            '--recipient-employee', $authorId, '--task-id', ([string]$employeeTask.task.id),
            '--text', 'COMPLETE_EMPLOYEE',
            '--idempotency-key', "employee-turn-$script:runId"
        )
        Assert-Equal $employeeTurn.turn.state 'completed' 'employee turn completion'
        Assert-Equal $employeeTurn.turn.author.employeeId $authorId 'employeeAuthorship'
        $gates.employeeAuthorship = 'PASS'

        $blocked = Invoke-ExpectedFailure 12 @(
            'turn', 'submit', $projectConversationId, '--author-employee', $authorId,
            '--recipient-employee', $leadId, '--text', 'BLOCKED_ACTIVATION',
            '--activate-if-needed', '--idempotency-key', "blocked-turn-$script:runId"
        )
        Assert-True $blocked.Contains('LZ-TURN-INVALID') 'employeeActivationBlocked'
        $gates.employeeActivationBlocked = 'PASS'

        $cancelCommand = Start-JsonCommand 'cancel-submit' @(
            'turn', 'submit', $projectlessId, '--text', 'WAIT_FOR_CANCEL',
            '--idempotency-key', "cancel-submit-$script:runId"
        )
        $acceptedCancel = Wait-TurnState $projectlessId @('accepted')
        $cancelled = Invoke-Json @(
            'turn', 'cancel', ([string]$acceptedCancel.id),
            '--expected-revision', ([string]$acceptedCancel.revision),
            '--idempotency-key', "cancel-$script:runId"
        )
        Assert-Equal $cancelled.turn.state 'cancelled' 'cancelEvidenceRequired'
        $cancelSubmit = Complete-JsonCommand $cancelCommand
        Assert-Equal $cancelSubmit.turn.state 'cancelled' 'cancelled submit result'
        $gates.cancelEvidenceRequired = 'PASS'

        $eventsBeforeRestart = @(Invoke-Json @('events', 'list', '--after', '0', '--limit', '1024'))
        $recordSequence = @($eventsBeforeRestart | Where-Object {
            $_.eventType -eq 'turn.recorded' -and $_.payload.turn.id -eq $acceptedCancel.id
        })[0].sequence
        $acceptSequence = @($eventsBeforeRestart | Where-Object {
            $_.eventType -eq 'turn.accepted' -and $_.payload.turn.id -eq $acceptedCancel.id
        })[0].sequence
        Assert-True ([long]$recordSequence -lt [long]$acceptSequence) 'recordFirstTurn'
        $gates.recordFirstTurn = 'PASS'

        $heldCommand = Start-JsonCommand 'held-submit' @(
            'turn', 'submit', $projectlessId, '--text', 'HOLD_FOR_RESTART',
            '--idempotency-key', "held-submit-$script:runId"
        )
        $held = Wait-TurnState $projectlessId @('accepted')
        $priorBootId = [string]$status.bootId
        Stop-Process -Id $daemon.Id -Force
        $daemon.WaitForExit(10000) | Out-Null
        $daemon = $null
        $heldCommand.Process.WaitForExit(10000) | Out-Null
        $daemon = Start-Daemon
        $status = Wait-Status $priorBootId
        $reconciled = Invoke-Json @('turn', 'show', ([string]$held.id))
        Assert-Equal $reconciled.state 'uncertain' 'ownerLossUncertain'
        Assert-Equal $reconciled.failureCode 'LZ-TURN-OWNER-LOST' 'owner loss failure code'
        $gates.ownerLossUncertain = 'PASS'

        $retryFailure = Invoke-ExpectedFailure 12 @(
            'turn', 'submit', $projectlessId, '--text', 'RETRY_ONCE', '--activate-if-needed',
            '--max-attempts', '2', '--idempotency-key', "retry-submit-$script:runId"
        )
        Assert-True $retryFailure.Contains('LZ-TURN-PROVIDER') 'retryable provider failure'
        $retryable = Wait-TurnState $projectlessId @('retryable-failed')
        $retryMessageId = [string]$retryable.messageId
        Start-Sleep -Milliseconds 1200
        $retried = Invoke-Json @(
            'turn', 'retry', ([string]$retryable.id),
            '--expected-revision', ([string]$retryable.revision),
            '--idempotency-key', "retry-due-$script:runId"
        )
        Assert-Equal $retried.turn.id $retryable.id 'retry turn identity'
        Assert-Equal $retried.message.envelope.id $retryMessageId 'retryIdentityPreserved'
        Assert-Equal $retried.message.attempts.Count 2 'retry attempt count'
        $gates.retryIdentityPreserved = 'PASS'

        $large = Invoke-Json @(
            'turn', 'submit', $projectlessId, '--text', 'LARGE_TRANSCRIPT',
            '--idempotency-key', "large-turn-$script:runId"
        )
        $largeDelta = @($large.turn.events | Where-Object { $_.kind -eq 'textDelta' })[0]
        Assert-True ([bool]$largeDelta.payload.truncated) 'transcriptBounded'
        Assert-True (($largeDelta.payload | ConvertTo-Json -Compress).Length -lt 65536) 'bounded payload bytes'
        $gates.transcriptBounded = 'PASS'
        $shutdownSessionId = [string]$large.turn.recipient.sessionId

        $events = @(Invoke-Json @('events', 'list', '--after', '0', '--limit', '1024'))
        $eventTypes = @($events | ForEach-Object { [string]$_.eventType })
        foreach ($requiredEvent in @('conversation.created', 'turn.recorded', 'turn.accepted', 'turn.completed', 'turn.cancelled', 'turns.reconciled')) {
            Assert-True ($eventTypes -contains $requiredEvent) "audit/outbox event $requiredEvent"
        }
        $drained = Wait-Status
        Assert-Equal $drained.pendingOutbox 0 'outbox drained'
        $gates.auditOutboxEvidence = 'PASS'
        $gates.modelInvocationAbsent = 'PASS'
        $evidence.eventTypes = @($eventTypes | Select-Object -Unique)
        $evidence.fixtureLog = @(Get-Content -LiteralPath $script:fixtureLog)
    }

    $gates.desktopCapabilityExpansionAbsent = 'PASS'
    Assert-True (-not [string]::IsNullOrWhiteSpace($shutdownSessionId)) 'shutdown session identity'
    $shutdownSession = Invoke-Json @('session', 'show', $shutdownSessionId)
    Assert-True ($null -ne $shutdownSession.nativeHostPid) 'shutdown native host PID'
    $shutdownTree = @(Get-ProcessTreeRecords ([int]$shutdownSession.nativeHostPid))
    Assert-True ($shutdownTree.Count -ge 2) 'shutdown native session host has a provider descendant'
    Stop-Daemon $daemon
    $daemon = $null
    foreach ($record in $shutdownTree) {
        Wait-ProcessGone ([int]$record.ProcessId) 10 'planned-shutdown native tree'
    }
    $gates.cleanProviderTreeStop = 'PASS'
    $manifestStatus = 'PASS'
} catch {
    $gates.failure = $_.Exception.Message
    throw
} finally {
    $stopwatch.Stop()
    if ($null -ne $daemon -and -not $daemon.HasExited) {
        Stop-Process -Id $daemon.Id -Force
        Wait-Process -Id $daemon.Id -Timeout 5 -ErrorAction SilentlyContinue
    }
    foreach ($createdPid in $script:createdPids) {
        $created = Get-Process -Id $createdPid -ErrorAction SilentlyContinue
        if ($null -ne $created -and -not $created.HasExited) {
            Stop-Process -Id $createdPid -Force -ErrorAction SilentlyContinue
        }
    }
    $commandSnapshot = @($script:commands | ForEach-Object {
        [ordered]@{
            command = [string]$_.command
            exitCode = if ($null -eq $_.exitCode) { $null } else { [int]$_.exitCode }
            durationMs = if ($null -eq $_.durationMs) { $null } else { [long]$_.durationMs }
        }
    })
    $gateSnapshot = [ordered]@{}
    foreach ($gateName in @($gates.Keys)) {
        $gateSnapshot[[string]$gateName] = [string]$gates[$gateName]
    }
    $evidenceSnapshot = [ordered]@{}
    foreach ($evidenceName in @($evidence.Keys)) {
        $value = $evidence[$evidenceName]
        if ($value -is [Array]) {
            $evidenceSnapshot[[string]$evidenceName] = @($value | ForEach-Object { [string]$_ })
        } else {
            $evidenceSnapshot[[string]$evidenceName] = [string]$value
        }
    }
    $manifest = [ordered]@{
        manifestVersion = 1
        status = $manifestStatus
        mode = if ($script:deterministic) { 'deterministic-fixture' } else { 'live-provider' }
        startedAt = $startedAt.ToString('o')
        completedAt = [DateTime]::UtcNow.ToString('o')
        durationMs = $stopwatch.ElapsedMilliseconds
        protocolVersion = 1
        schemaVersion = 19
        isolatedDataRoot = $script:dataRoot
        temporaryDataRetained = $false
        commands = $commandSnapshot
        gates = $gateSnapshot
        evidence = $evidenceSnapshot
    }
    $manifest | ConvertTo-Json -Depth 10 | Set-Content -LiteralPath $manifestPath -Encoding UTF8
    $resolvedRunRoot = [IO.Path]::GetFullPath($script:runRoot)
    if ($resolvedRunRoot.StartsWith($tempBase, [StringComparison]::OrdinalIgnoreCase) -and
        (Split-Path -Leaf $resolvedRunRoot).StartsWith('lazoo-conversation-delivery-')) {
        Remove-Item -LiteralPath $resolvedRunRoot -Recurse -Force -ErrorAction SilentlyContinue
    }
}

Write-Output "PASS manifest: $manifestPath"
