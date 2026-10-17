param(
    [switch]$Build,
    [int]$TurnTimeoutSeconds = 240,
    [int]$CancelTimeoutSeconds = 30
)

$ErrorActionPreference = 'Stop'
Set-StrictMode -Version Latest

# LIVE acceptance for Claude Code (PROJECT-BRIEF section 21.3): one daemon lifetime on a disposable
# data root, one Claude Code employee under full autonomy, and exactly THREE real model turns on the
# owner's native subscription: messaging, cancellation, and resume after a stop. It sends real
# prompts, so it is never part of scripts/gate.ps1 (the gate's $excluded list names it). It never
# reads or writes the owner's Claude configuration; the daemon's environment is scrubbed of every
# API credential, cloud selector and outer Claude Code session variable so the only channel left is
# the native login. One run, at most three live turns, no retries.

$prompts = [ordered]@{
    messaging = 'Remember the code word PELICAN for later in this conversation. Reply with exactly LAZOO_CLAUDE_OK and nothing else.'
    cancellation = 'Count slowly from 1 to 400, one number per line, with no other text.'
    resume = 'Reply with exactly the code word I asked you to remember, and nothing else.'
}
$maxLiveTurns = 3

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

function ConvertTo-SafeText([string]$Message, [int]$Limit = 600) {
    if ([string]::IsNullOrWhiteSpace($Message)) { return '' }
    $safe = $Message -replace '(?i)(api[_-]?key|token|authorization|secret|password|bearer)\s*[:=]?\s*\S+', '$1=<redacted>'
    $safe = $safe -replace '[A-Z0-9._%+-]+@[A-Z0-9.-]+\.[A-Z]{2,}', '<redacted-email>'
    if ($safe.Length -gt $Limit) { $safe = $safe.Substring(0, $Limit) }
    return $safe
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

# Runs lazooctl --json and returns the exit code and text. Polling calls pass -Quiet so the manifest
# keeps the commands that matter and a poll count instead of hundreds of identical lines.
function Invoke-Ctl([string[]]$Arguments, [switch]$Quiet) {
    $previousRoot = Set-DataRoot $script:dataRoot
    $previousPreference = $ErrorActionPreference
    try {
        $ErrorActionPreference = 'Continue'
        $timer = [Diagnostics.Stopwatch]::StartNew()
        $captured = @(& $script:ctlExe --json @Arguments 2>&1)
        $exitCode = $LASTEXITCODE
        $timer.Stop()
        if ($Quiet) {
            $script:polls++
        } else {
            $script:commands.Add([ordered]@{
                command = "lazooctl --json $($Arguments -join ' ')"
                exitCode = $exitCode
                durationMs = $timer.ElapsedMilliseconds
            })
        }
        return [pscustomobject]@{ ExitCode = $exitCode; Text = ($captured -join [Environment]::NewLine) }
    } finally {
        $ErrorActionPreference = $previousPreference
        Restore-DataRoot $previousRoot
    }
}

function Invoke-Json([string[]]$Arguments, [switch]$Quiet) {
    $result = Invoke-Ctl $Arguments -Quiet:$Quiet
    if ($result.ExitCode -ne 0) {
        throw "lazooctl $($Arguments -join ' ') exited $($result.ExitCode) with: $(ConvertTo-SafeText $result.Text)"
    }
    return ($result.Text | ConvertFrom-Json)
}

function Start-Daemon {
    $startInfo = [Diagnostics.ProcessStartInfo]::new()
    $startInfo.FileName = $script:daemonExe
    $startInfo.UseShellExecute = $false
    $startInfo.CreateNoWindow = $true
    $startInfo.Environment['LAZOO_DATA_ROOT'] = $script:dataRoot
    # Only the names are kept; values are never read into the manifest.
    foreach ($name in @($startInfo.Environment.Keys)) {
        if ($name -match '^(ANTHROPIC_|CLAUDECODE$|CLAUDE_CODE_(?!GIT_BASH_PATH$)|CLAUDE_PID$|CLAUDE_EFFORT$|CLAUDE_PLUGIN_|CODEX_THREAD_ID$|CODEX_SESSION_ID$|AI_AGENT$)') {
            [void]$startInfo.Environment.Remove($name)
            $script:scrubbedEnvironment.Add([string]$name)
        }
    }
    $process = [Diagnostics.Process]::new()
    $process.StartInfo = $startInfo
    Assert-True $process.Start() 'daemon process start'
    $script:createdPids.Add($process.Id)
    return $process
}

function Wait-Status([int]$TimeoutSeconds = 30) {
    $deadline = [DateTime]::UtcNow.AddSeconds($TimeoutSeconds)
    $lastError = ''
    while ([DateTime]::UtcNow -lt $deadline) {
        $result = Invoke-Ctl @('status') -Quiet
        if ($result.ExitCode -eq 0) { return ($result.Text | ConvertFrom-Json) }
        $lastError = $result.Text
        Start-Sleep -Milliseconds 200
    }
    throw "daemon did not become ready: $(ConvertTo-SafeText $lastError)"
}

# A blocking lazooctl call run in the background so the script can act while a turn is in flight.
function Start-BackgroundCtl([string]$Name, [string[]]$Arguments) {
    $stdoutPath = Join-Path $script:runRoot "$Name.stdout.json"
    $stderrPath = Join-Path $script:runRoot "$Name.stderr.txt"
    $previousRoot = Set-DataRoot $script:dataRoot
    try {
        # Start-Process joins its arguments with spaces, so each one is quoted here.
        $quoted = @(@('--json') + $Arguments | ForEach-Object { '"' + (([string]$_) -replace '"', '\"') + '"' })
        $process = Start-Process -FilePath $script:ctlExe -ArgumentList $quoted `
            -RedirectStandardOutput $stdoutPath -RedirectStandardError $stderrPath `
            -PassThru -WindowStyle Hidden
        $script:createdPids.Add($process.Id)
        return [pscustomobject]@{
            Name = $Name
            Process = $process
            StdoutPath = $stdoutPath
            StderrPath = $stderrPath
            Arguments = $Arguments
            Timer = [Diagnostics.Stopwatch]::StartNew()
        }
    } finally {
        Restore-DataRoot $previousRoot
    }
}

# Waits for a background call; a timeout is a recorded outcome, never a hang.
function Wait-BackgroundCtl($Command, [int]$TimeoutSeconds) {
    $finished = $Command.Process.WaitForExit($TimeoutSeconds * 1000)
    $Command.Timer.Stop()
    if (-not $finished) {
        $script:commands.Add([ordered]@{
            command = "lazooctl --json $($Command.Arguments -join ' ')"
            exitCode = $null
            durationMs = $Command.Timer.ElapsedMilliseconds
        })
        return [pscustomobject]@{ TimedOut = $true; ExitCode = $null; Value = $null; Text = ''; DurationMs = $Command.Timer.ElapsedMilliseconds }
    }
    $Command.Process.WaitForExit()
    $Command.Process.Refresh()
    $exitCode = [int]$Command.Process.ExitCode
    $stdout = if (Test-Path -LiteralPath $Command.StdoutPath) { Get-Content -LiteralPath $Command.StdoutPath -Raw } else { '' }
    $stderr = if (Test-Path -LiteralPath $Command.StderrPath) { Get-Content -LiteralPath $Command.StderrPath -Raw } else { '' }
    if ($null -eq $stdout) { $stdout = '' }
    if ($null -eq $stderr) { $stderr = '' }
    $script:commands.Add([ordered]@{
        command = "lazooctl --json $($Command.Arguments -join ' ')"
        exitCode = $exitCode
        durationMs = $Command.Timer.ElapsedMilliseconds
    })
    $value = $null
    if ($exitCode -eq 0 -and $stdout.Trim() -ne '') { $value = $stdout | ConvertFrom-Json }
    return [pscustomobject]@{
        TimedOut = $false; ExitCode = $exitCode; Value = $value
        Text = (ConvertTo-SafeText ($stderr + ' ' + $stdout)); DurationMs = $Command.Timer.ElapsedMilliseconds
    }
}

function Test-AuthenticationText([string]$Text) {
    return ($Text -match '(?i)auth|log ?in|logged|credential|unauthori[sz]ed|401|subscription|expired|api key')
}

function Submit-LiveTurn([string]$Name, [string]$ConversationId, [string]$Prompt) {
    if ($script:liveTurns -ge $maxLiveTurns) { throw "refusing a live turn beyond the cap of $maxLiveTurns" }
    $script:liveTurns++
    return (Start-BackgroundCtl "$Name-submit" @(
        'turn', 'submit', $ConversationId, '--authority-mode', 'full-autonomy', '--text', $Prompt,
        '--activate-if-needed', '--idempotency-key', "$Name-$script:runId"
    ))
}

function Complete-LiveTurn($Command, [string]$Name) {
    $result = Wait-BackgroundCtl $Command $TurnTimeoutSeconds
    $script:turnDurations[$Name] = $result.DurationMs
    if ($result.TimedOut) {
        $script:outcome = 'TIMEOUT'
        throw "live turn '$Name' did not finish within $TurnTimeoutSeconds seconds"
    }
    if ($result.ExitCode -ne 0) {
        $script:outcome = if (Test-AuthenticationText $result.Text) { 'BLOCKED_AUTHENTICATION' } else { 'TURN_FAILED' }
        throw "live turn '$Name' failed with lazooctl exit $($result.ExitCode): $($result.Text)"
    }
    return $result.Value
}

function Get-TurnOutput($TurnResult) {
    $completed = @($TurnResult.turn.events | Where-Object { $_.kind -eq 'turnCompleted' })
    if ($completed.Count -eq 0) { return $null }
    return [string]$completed[-1].payload.output
}

function Get-TurnList([string]$ConversationId) {
    return @(Invoke-Json @('turn', 'list', $ConversationId, '--after-sequence', '0', '--limit', '100') -Quiet)
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
        if (-not $seen.Add($processId)) { continue }
        $record = $processes | Where-Object { $_.ProcessId -eq $processId } | Select-Object -First 1
        if ($null -ne $record) { $records.Add($record) }
        foreach ($child in @($processes | Where-Object { $_.ParentProcessId -eq $processId })) {
            $pending.Enqueue([int]$child.ProcessId)
        }
    }
    return @($records)
}

function Wait-ProcessGone([int]$ProcessId, [int]$TimeoutSeconds, [string]$Label) {
    $deadline = [DateTime]::UtcNow.AddSeconds($TimeoutSeconds)
    while ([DateTime]::UtcNow -lt $deadline) {
        if ($null -eq (Get-ProcessRecord $ProcessId)) { return }
        Start-Sleep -Milliseconds 100
    }
    throw "$Label process $ProcessId remained alive after $TimeoutSeconds seconds"
}

function Limit-Output([string]$Text) {
    if ($null -eq $Text) { return $null }
    $trimmed = $Text.Trim()
    if ($trimmed.Length -gt 400) { return $trimmed.Substring(0, 400) }
    return $trimmed
}

$repoRoot = (Resolve-Path (Join-Path $PSScriptRoot '..\..')).Path
$releaseDir = Join-Path $repoRoot 'target\release'
$script:daemonExe = Join-Path $releaseDir 'lazood.exe'
$script:ctlExe = Join-Path $releaseDir 'lazooctl.exe'
$artifactDir = Join-Path $repoRoot 'artifacts\acceptance'
$manifestPath = Join-Path $artifactDir 'claude-live-lifecycle-manifest.json'
$script:runId = [Guid]::NewGuid().ToString('N')
$tempBase = [IO.Path]::GetFullPath([IO.Path]::GetTempPath())
$script:runRoot = [IO.Path]::GetFullPath((Join-Path $tempBase "lazoo-claude-live-$script:runId"))
if (-not $script:runRoot.StartsWith($tempBase, [StringComparison]::OrdinalIgnoreCase)) {
    throw 'acceptance run root is outside the system temporary directory'
}
$script:dataRoot = Join-Path $script:runRoot 'data'
$startedAt = [DateTime]::UtcNow
$stopwatch = [Diagnostics.Stopwatch]::StartNew()
$script:commands = [Collections.Generic.List[object]]::new()
$script:createdPids = [Collections.Generic.List[int]]::new()
$script:scrubbedEnvironment = [Collections.Generic.List[string]]::new()
$script:turnDurations = [ordered]@{}
$script:polls = 0
$script:liveTurns = 0
$script:outcome = 'FAILED_ASSERTION'
$gates = [ordered]@{
    authentication = 'NOT_RUN'
    messaging = 'NOT_RUN'
    cancellation = 'NOT_RUN'
    resume = 'NOT_RUN'
    plannedShutdown = 'NOT_RUN'
}
$currentGate = 'authentication'
$evidence = [ordered]@{}
$turns = [ordered]@{}
$daemon = $null
$manifestStatus = 'FAIL'
$failureMessage = ''
$claudeVersion = ''

New-Item -ItemType Directory -Force -Path $artifactDir, $script:dataRoot | Out-Null

try {
    if ($Build) {
        $cargoExe = (Get-Command cargo -CommandType Application -ErrorAction Stop).Source
        $timer = [Diagnostics.Stopwatch]::StartNew()
        & $cargoExe build --workspace --release --locked
        $buildExit = $LASTEXITCODE
        $timer.Stop()
        $script:commands.Add([ordered]@{ command = 'cargo build --workspace --release --locked'; exitCode = $buildExit; durationMs = $timer.ElapsedMilliseconds })
        Assert-Equal $buildExit 0 'locked release build'
    }
    foreach ($binary in @($script:daemonExe, $script:ctlExe)) {
        Assert-True (Test-Path -LiteralPath $binary) "binary exists: $binary"
    }
    $claudeCommand = Get-Command claude -CommandType Application -ErrorAction SilentlyContinue | Select-Object -First 1
    Assert-True ($null -ne $claudeCommand) 'claude executable on PATH'
    $claudeVersion = ((& $claudeCommand.Source --version 2>&1) -join ' ').Trim()

    $daemon = Start-Daemon
    $status = Wait-Status
    Assert-Equal $status.schemaVersion 19 'schema 19'

    # Authentication: a read-only probe, no model prompt. A known-missing login ends the run here.
    $probe = Invoke-Json @('provider', 'probe', 'claude-code')
    $evidence.authentication = [ordered]@{
        native = [string]$probe.authentication.native
        external = [string]$probe.authentication.external
        verification = [string]$probe.verification
        version = [string]$probe.version
    }
    if ($probe.authentication.native -in @('needs-login', 'blocked') -and $probe.authentication.external -ne 'ready') {
        $script:outcome = 'BLOCKED_AUTHENTICATION'
        throw 'Claude Code reports no ready native login and no ready external credential; no model prompt was sent'
    }
    $gates.authentication = 'PASS'

    $employee = Invoke-Json @(
        'employee', 'create', 'Cleo', '--provider', 'claude-code', '--role', 'Lead',
        '--authority-mode', 'full-autonomy', '--idle-timeout-seconds', '0',
        '--idempotency-key', "lead-$script:runId"
    )
    $employeeId = [string]$employee.employee.id
    $conversation = Invoke-Json @(
        'conversation', 'create', $employeeId, '--title', 'Claude live lifecycle',
        '--idempotency-key', "conversation-$script:runId"
    )
    $conversationId = [string]$conversation.conversation.id

    # Turn 1: messaging.
    $currentGate = 'messaging'
    $first = Complete-LiveTurn (Submit-LiveTurn 'turn1' $conversationId $prompts.messaging) 'turn1'
    $firstOutput = Get-TurnOutput $first
    $turns.messaging = [ordered]@{
        prompt = $prompts.messaging
        state = [string]$first.turn.state
        output = Limit-Output $firstOutput
        durationMs = $script:turnDurations['turn1']
    }
    Assert-Equal ([string]$first.turn.state) 'completed' 'turn 1 state'
    Assert-True ($null -ne $firstOutput) 'turn 1 carries a turnCompleted event with output'
    Assert-Equal $firstOutput.Trim() 'LAZOO_CLAUDE_OK' 'turn 1 trimmed output'
    $sessionOneId = [string]$first.turn.recipient.sessionId
    $sessionOne = Invoke-Json @('session', 'show', $sessionOneId)
    $nativeSessionId = [string]$sessionOne.nativeSessionId
    Assert-True (-not [string]::IsNullOrWhiteSpace($nativeSessionId) -and -not $nativeSessionId.StartsWith('pending:')) 'turn 1 native session id recorded'
    $evidence.sessionId = $sessionOneId
    $evidence.nativeSessionId = $nativeSessionId
    $gates.messaging = 'PASS'

    # Turn 2: cancellation. Submit blocks until the turn ends, so it runs in the background and the
    # script cancels as soon as the turn is accepted.
    $currentGate = 'cancellation'
    $knownTurnIds = @(Get-TurnList $conversationId | ForEach-Object { [string]$_.id })
    $second = Submit-LiveTurn 'turn2' $conversationId $prompts.cancellation
    $acceptDeadline = [DateTime]::UtcNow.AddSeconds($TurnTimeoutSeconds)
    $secondTurn = $null
    while ([DateTime]::UtcNow -lt $acceptDeadline) {
        $fresh = @(Get-TurnList $conversationId | Where-Object { $knownTurnIds -notcontains [string]$_.id })
        if ($fresh.Count -gt 0) {
            $secondTurn = Invoke-Json @('turn', 'show', ([string]$fresh[0].id)) -Quiet
            if (@('accepted', 'completed', 'terminal-failed', 'retryable-failed', 'cancelled', 'uncertain') -contains [string]$secondTurn.state) { break }
        }
        Start-Sleep -Milliseconds 100
    }
    if ($null -eq $secondTurn -or [string]$secondTurn.state -ne 'accepted') {
        $seen = if ($null -eq $secondTurn) { 'no turn' } else { [string]$secondTurn.state }
        $script:outcome = if ($null -eq $secondTurn) { 'TIMEOUT' } else { 'TURN_FAILED' }
        throw "turn 2 never reached 'accepted' before a cancel could be requested (saw: $seen)"
    }
    $secondTurnId = [string]$secondTurn.id
    $cancelTimer = [Diagnostics.Stopwatch]::StartNew()
    $cancelResult = $null
    for ($attempt = 1; $attempt -le 3; $attempt++) {
        $current = Invoke-Json @('turn', 'show', $secondTurnId) -Quiet
        $cancelResult = Invoke-Ctl @(
            'turn', 'cancel', $secondTurnId, '--expected-revision', ([string]$current.revision),
            '--idempotency-key', "cancel-$attempt-$script:runId"
        )
        if ($cancelResult.ExitCode -eq 0) { break }
        if ($cancelResult.Text -notmatch '(?i)revision') { break }
    }
    $cancelTimer.Stop()
    if ($cancelResult.ExitCode -ne 0) {
        # A refused or uncertain cancel is a recorded outcome: keep what the daemon stored about it.
        $script:outcome = 'CANCEL_NOT_CONFIRMED'
        $afterRefusal = Invoke-Ctl @('turn', 'show', $secondTurnId) -Quiet
        $sessionRefusal = Invoke-Ctl @('session', 'show', $sessionOneId) -Quiet
        $refusedTurn = if ($afterRefusal.ExitCode -eq 0) { $afterRefusal.Text | ConvertFrom-Json } else { $null }
        $refusedSession = if ($sessionRefusal.ExitCode -eq 0) { $sessionRefusal.Text | ConvertFrom-Json } else { $null }
        $turns.cancellation = [ordered]@{
            prompt = $prompts.cancellation
            cancelExitCode = $cancelResult.ExitCode
            cancelError = ConvertTo-SafeText $cancelResult.Text 500
            cancelCallMs = $cancelTimer.ElapsedMilliseconds
            turnStateAfterCancel = if ($null -eq $refusedTurn) { $null } else { [string]$refusedTurn.state }
            turnFailureCode = if ($null -eq $refusedTurn) { $null } else { [string]($refusedTurn.PSObject.Properties["failureCode"].Value) }
            turnFailureMessage = if ($null -eq $refusedTurn) { $null } else { ConvertTo-SafeText ([string]($refusedTurn.PSObject.Properties["failureMessage"].Value)) 500 }
            sessionStateAfterCancel = if ($null -eq $refusedSession) { $null } else { [string]$refusedSession.state }
        }
        throw "turn cancel exit $($cancelResult.ExitCode): $(ConvertTo-SafeText $cancelResult.Text 300)"
    }
    $cancelled = $cancelResult.Text | ConvertFrom-Json
    Assert-Equal ([string]$cancelled.turn.state) 'cancelled' 'cancel result turn state'
    $secondDone = Wait-BackgroundCtl $second $CancelTimeoutSeconds
    $script:turnDurations['turn2'] = $secondDone.DurationMs
    Assert-True (-not $secondDone.TimedOut) "the cancelled submit returned within $CancelTimeoutSeconds seconds"
    $secondFinal = Invoke-Json @('turn', 'show', $secondTurnId)
    Assert-Equal ([string]$secondFinal.state) 'cancelled' 'turn 2 final state'
    $sessionAfterCancel = Invoke-Json @('session', 'show', $sessionOneId)
    $events = @(Invoke-Json @('events', 'list', '--after', '0', '--limit', '1024'))
    $completedForSecond = @($events | Where-Object {
        $_.eventType -eq 'turn.completed' -and (($_.payload | ConvertTo-Json -Compress -Depth 12) -match [regex]::Escape($secondTurnId))
    })
    $cancelledForSecond = @($events | Where-Object {
        $_.eventType -eq 'turn.cancelled' -and (($_.payload | ConvertTo-Json -Compress -Depth 12) -match [regex]::Escape($secondTurnId))
    })
    $turns.cancellation = [ordered]@{
        prompt = $prompts.cancellation
        state = [string]$secondFinal.state
        cancelRequestToCancelledMs = $cancelTimer.ElapsedMilliseconds
        submitReturnedMs = $secondDone.DurationMs
        submitExitCode = $secondDone.ExitCode
        turnCompletedEvents = $completedForSecond.Count
        turnCancelledEvents = $cancelledForSecond.Count
        sessionStateAfterCancel = [string]$sessionAfterCancel.state
        sessionAfterCancel = if ([string]$sessionAfterCancel.state -eq 'idle') { 'STILL_USABLE_IDLE' } else { 'DAEMON_MOVED_SESSION_TO_' + ([string]$sessionAfterCancel.state).ToUpperInvariant() }
    }
    Assert-Equal $completedForSecond.Count 0 'turn.completed events for the cancelled turn'
    Assert-True ($cancelledForSecond.Count -ge 1) 'a turn.cancelled event for the cancelled turn'
    Assert-True (@('idle', 'stopping', 'suspended') -contains [string]$sessionAfterCancel.state) "session after cancel is idle, stopping or suspended (was '$($sessionAfterCancel.state)')"
    $gates.cancellation = 'PASS'

    # Step 4: stop the session (unless the daemon already did) and require suspended.
    $currentGate = 'resume'
    $firstHostPid = $sessionAfterCancel.nativeHostPid
    $firstTree = if ($null -ne $firstHostPid) { @(Get-ProcessTreeRecords ([int]$firstHostPid)) } else { @() }
    $sessionNow = $sessionAfterCancel
    if ([string]$sessionNow.state -eq 'idle') {
        $stopped = Invoke-Json @(
            'session', 'stop', $sessionOneId, '--expected-revision', ([string]$sessionNow.revision),
            '--idempotency-key', "stop-$script:runId"
        )
        $evidence.stoppedBy = 'lazooctl session stop'
        Assert-Equal ([string]$stopped.session.state) 'suspended' 'session stop result state'
    } else {
        $evidence.stoppedBy = "daemon (state '$($sessionNow.state)' after cancel)"
    }
    $suspendDeadline = [DateTime]::UtcNow.AddSeconds(20)
    do {
        $sessionNow = Invoke-Json @('session', 'show', $sessionOneId) -Quiet
        if ([string]$sessionNow.state -eq 'suspended') { break }
        Start-Sleep -Milliseconds 200
    } while ([DateTime]::UtcNow -lt $suspendDeadline)
    $shownStopped = Invoke-Json @('session', 'show', $sessionOneId)
    Assert-Equal ([string]$shownStopped.state) 'suspended' 'session state after stop'
    Assert-Equal ([string]$shownStopped.nativeSessionId) $nativeSessionId 'native session id retained across the stop'
    $survivors = @($firstTree | Where-Object { $null -ne (Get-ProcessRecord ([int]$_.ProcessId)) })
    $evidence.firstSessionNativeTreeProcessesAtStop = @($firstTree).Count
    $evidence.firstSessionNativeTreeSurvivors = $survivors.Count

    # Turn 3: resume.
    $third = Complete-LiveTurn (Submit-LiveTurn 'turn3' $conversationId $prompts.resume) 'turn3'
    $thirdOutput = Get-TurnOutput $third
    $sessionThreeId = [string]$third.turn.recipient.sessionId
    $sessionThree = Invoke-Json @('session', 'show', $sessionThreeId)
    $events = @(Invoke-Json @('events', 'list', '--after', '0', '--limit', '1024'))
    $resumedEvents = @($events | Where-Object { $_.eventType -eq 'session.resumed' })
    $skippedEvents = @($events | Where-Object { $_.eventType -eq 'session.resume.skipped' })
    $turns.resume = [ordered]@{
        prompt = $prompts.resume
        state = [string]$third.turn.state
        output = Limit-Output $thirdOutput
        durationMs = $script:turnDurations['turn3']
        newSessionId = $sessionThreeId
        newNativeSessionId = [string]$sessionThree.nativeSessionId
        sessionResumedEvents = $resumedEvents.Count
        sessionResumeSkippedEvents = $skippedEvents.Count
    }
    $evidence.resumedPayloads = @($resumedEvents | ForEach-Object { $_.payload | ConvertTo-Json -Compress -Depth 8 })
    $evidence.resumeSkippedPayloads = @($skippedEvents | ForEach-Object { $_.payload | ConvertTo-Json -Compress -Depth 8 })
    Assert-Equal $skippedEvents.Count 0 "session.resume.skipped events (payload: $(ConvertTo-SafeText ($evidence.resumeSkippedPayloads -join ' | ')))"
    Assert-Equal ([string]$third.turn.state) 'completed' 'turn 3 state'
    Assert-True ($sessionThreeId -ne $sessionOneId) 'turn 3 ran on a new session record'
    Assert-Equal ([string]$sessionThree.nativeSessionId) $nativeSessionId 'resumed native session id equals the recorded one'
    Assert-Equal $resumedEvents.Count 1 'session.resumed event count'
    Assert-Equal ([string]$resumedEvents[0].payload.priorSessionId) $sessionOneId 'session.resumed names the prior session'
    Assert-Equal ([string]$resumedEvents[0].payload.nativeSessionId) $nativeSessionId 'session.resumed names the native session'
    Assert-True ($null -ne $thirdOutput) 'turn 3 carries a turnCompleted event with output'
    Assert-Equal $thirdOutput.Trim() 'PELICAN' 'turn 3 trimmed output'
    $gates.resume = 'PASS'

    # Planned shutdown: the native tree of the live (resumed) session must be gone afterwards.
    $currentGate = 'plannedShutdown'
    Assert-True ($null -ne $sessionThree.nativeHostPid) 'resumed session native host PID'
    $shutdownTree = @(Get-ProcessTreeRecords ([int]$sessionThree.nativeHostPid))
    Assert-True ($shutdownTree.Count -ge 1) 'resumed session native host is running'
    $evidence.shutdownTreeProcesses = $shutdownTree.Count
    Invoke-Json @('system', 'shutdown') | Out-Null
    Assert-True ($daemon.WaitForExit(20000)) 'daemon exited after planned shutdown'
    Assert-Equal $daemon.ExitCode 0 'planned daemon exit code'
    $daemon = $null
    foreach ($record in $shutdownTree) {
        Wait-ProcessGone ([int]$record.ProcessId) 15 'planned-shutdown native tree'
    }
    $gates.plannedShutdown = 'PASS'
    $manifestStatus = 'PASS'
    $script:outcome = 'PASS'
} catch {
    $failureMessage = ConvertTo-SafeText $_.Exception.Message 900
    $gates[$currentGate] = 'FAIL'
} finally {
    $stopwatch.Stop()
    if ($null -ne $daemon -and -not $daemon.HasExited) {
        Stop-Process -Id $daemon.Id -Force -ErrorAction SilentlyContinue
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
    $manifest = [ordered]@{
        manifestVersion = 1
        status = $manifestStatus
        outcome = $script:outcome
        failure = if ($failureMessage -eq '') { $null } else { $failureMessage }
        mode = 'live-native-subscription'
        schemaVersion = 19
        startedAt = $startedAt.ToString('o')
        completedAt = [DateTime]::UtcNow.ToString('o')
        durationMs = $stopwatch.ElapsedMilliseconds
        provider = 'claude-code'
        claudeVersion = $claudeVersion
        authenticationChannel = 'native-subscription'
        authenticationChannelNote = 'ANTHROPIC_*, CLAUDE_CODE_* and outer-session variables were removed from the daemon environment (names only recorded); the owner Claude configuration was neither read nor changed'
        scrubbedEnvironmentVariableNames = @($script:scrubbedEnvironment | Sort-Object -Unique)
        liveTurnsSent = $script:liveTurns
        liveTurnCap = $maxLiveTurns
        turnTimeoutSeconds = $TurnTimeoutSeconds
        isolatedDataRoot = $script:dataRoot
        temporaryDataRetained = $false
        prompts = $prompts
        turns = $turns
        turnDurationsMs = $script:turnDurations
        pollCalls = $script:polls
        commands = $commandSnapshot
        gates = $gates
        evidence = $evidence
        boundaries = [ordered]@{
            permissions = 'NOT_APPLICABLE_FULL_AUTONOMY_ONLY'
            deterministicGate = 'EXCLUDED'
            userClaudeConfiguration = 'UNTOUCHED'
            forkOfLiveSession = 'NOT_RUN_SECTION_27_DECISION'
        }
    }
    $manifest | ConvertTo-Json -Depth 12 | Set-Content -LiteralPath $manifestPath -Encoding UTF8
    $resolvedRunRoot = [IO.Path]::GetFullPath($script:runRoot)
    if ($resolvedRunRoot.StartsWith($tempBase, [StringComparison]::OrdinalIgnoreCase) -and
        (Split-Path -Leaf $resolvedRunRoot).StartsWith('lazoo-claude-live-')) {
        Remove-Item -LiteralPath $resolvedRunRoot -Recurse -Force -ErrorAction SilentlyContinue
    }
}

if ($manifestStatus -ne 'PASS') {
    Write-Output "FAIL ($script:outcome) at gate '$currentGate': $failureMessage"
    Write-Output "manifest: $manifestPath"
    exit 1
}
Write-Output "PASS manifest: $manifestPath"
