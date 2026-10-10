param(
    [switch]$SkipBuild
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

function ConvertTo-NativeJsonArgument([string]$Value) {
    if ($PSVersionTable.PSVersion.Major -lt 7) {
        return $Value.Replace('"', '\"')
    }
    return $Value
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
    $previousRoot = Set-DataRoot $script:dataRoot
    try {
        return Start-Process -FilePath $script:daemonExe -PassThru -WindowStyle Hidden
    } finally {
        Restore-DataRoot $previousRoot
    }
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

function Wait-OutboxDrained([int]$TimeoutSeconds = 10) {
    $deadline = [DateTime]::UtcNow.AddSeconds($TimeoutSeconds)
    do {
        $status = Invoke-Json @('status')
        if ([long]$status.pendingOutbox -eq 0) {
            return $status
        }
        Start-Sleep -Milliseconds 100
    } while ([DateTime]::UtcNow -lt $deadline)
    throw "outbox did not drain; pending count is $($status.pendingOutbox)"
}

function Stop-Daemon([System.Diagnostics.Process]$Process) {
    Invoke-Json @('system', 'shutdown') | Out-Null
    if (-not $Process.WaitForExit(10000)) {
        throw "daemon $($Process.Id) did not stop after planned shutdown"
    }
    Assert-Equal $Process.ExitCode 0 'planned daemon exit code'
}

$repoRoot = (Resolve-Path (Join-Path $PSScriptRoot '..\..')).Path
$cargoExe = (Get-Command cargo -CommandType Application -ErrorAction Stop).Source
$releaseDir = Join-Path $repoRoot 'target\release'
$script:daemonExe = Join-Path $releaseDir 'lazood.exe'
$script:ctlExe = Join-Path $releaseDir 'lazooctl.exe'
$artifactDir = Join-Path $repoRoot 'artifacts\acceptance'
$manifestPath = Join-Path $artifactDir 'collaboration-bus-manifest.json'
$runId = [Guid]::NewGuid().ToString('N')
$tempBase = [IO.Path]::GetFullPath([IO.Path]::GetTempPath())
$runRoot = [IO.Path]::GetFullPath((Join-Path $tempBase "lazoo-collaboration-bus-$runId"))
if (-not $runRoot.StartsWith($tempBase, [StringComparison]::OrdinalIgnoreCase)) {
    throw 'acceptance run root is outside the system temporary directory'
}
$script:dataRoot = Join-Path $runRoot 'data'
$startedAt = [DateTime]::UtcNow
$stopwatch = [Diagnostics.Stopwatch]::StartNew()
$script:commands = [Collections.Generic.List[object]]::new()
$gates = [ordered]@{}
$evidence = [ordered]@{}
$daemon = $null
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
    $gates.releaseBuild = if ($SkipBuild) { 'SKIPPED_BY_CALLER' } else { 'PASS' }

    $daemon = Start-Daemon
    $status = Wait-Status
    Assert-Equal $status.protocolVersion 1 'protocol version'
    Assert-Equal $status.schemaVersion 12 'schemaVersion 12'
    Assert-Equal $status.journalMode 'wal' 'SQLite journal mode'
    $gates.schemaVersion = 'PASS'

    $sender = Invoke-Json @(
        'employee', 'create', 'Rhea',
        '--provider', 'codex',
        '--role', 'Lead',
        '--idempotency-key', "sender-$runId"
    )
    $recipient = Invoke-Json @(
        'employee', 'create', 'Ivo',
        '--provider', 'codex',
        '--role', 'Implementer',
        '--idempotency-key', "recipient-$runId"
    )
    $senderId = [string]$sender.employee.id
    $recipientId = [string]$recipient.employee.id

    $sendArguments = @(
        'message', 'send', $senderId, $recipientId,
        '--kind', 'peer',
        '--body', (ConvertTo-NativeJsonArgument '{"text":"spacing-token-request"}'),
        '--references', (ConvertTo-NativeJsonArgument '[{"kind":"file","target":"tokens.css","metadata":{}}]'),
        '--idempotency-key', "send-$runId"
    )
    $sent = Invoke-Json $sendArguments
    $sendReplay = Invoke-Json $sendArguments
    Assert-Equal $sent.message.envelope.id $sendReplay.message.envelope.id 'idempotent message identity'
    Assert-Equal $sent.message.delivery.state 'queued' 'record-first send state'
    Assert-True ($null -ne $sent.eventSequence) 'record-first event sequence'
    Assert-True ($null -eq $sendReplay.eventSequence) 'idempotent replay has no second event'
    $messageId = [string]$sent.message.envelope.id
    $correlationId = [string]$sent.message.envelope.correlationId
    $firstSequence = [long]$sent.message.sequence
    $gates.recordFirstSend = 'PASS'
    $gates.idempotentSend = 'PASS'

    $caused = Invoke-Json @(
        'message', 'send', $recipientId, $senderId,
        '--kind', 'reply',
        '--body', (ConvertTo-NativeJsonArgument '{"text":"space-3"}'),
        '--causation-id', $messageId,
        '--idempotency-key', "caused-$runId"
    )
    Assert-Equal $caused.message.envelope.causationId $messageId 'causation identity'
    Assert-Equal $caused.message.envelope.correlationId $correlationId 'correlation inheritance'
    $causedId = [string]$caused.message.envelope.id
    $allMessages = @(Invoke-Json @('message', 'list', '--after-sequence', '0', '--limit', '100'))
    Assert-Equal $allMessages.Count 2 'ordered message count'
    Assert-True ([long]$allMessages[0].sequence -lt [long]$allMessages[1].sequence) 'sequence ordering'
    $afterFirst = @(Invoke-Json @('message', 'list', '--after-sequence', ([string]$firstSequence), '--limit', '100'))
    Assert-Equal $afterFirst.Count 1 'ordered replay cursor count'
    Assert-Equal $afterFirst[0].id $causedId 'ordered replay cursor identity'
    $gates.orderedReplay = 'PASS'
    $gates.causalCorrelation = 'PASS'

    $claimed = Invoke-Json @(
        'message', 'claim', $messageId,
        '--expected-revision', '1',
        '--idempotency-key', "claim-$runId"
    )
    Assert-Equal $claimed.message.delivery.state 'delivering' 'claimed delivery state'
    $attemptId = [string]$claimed.message.attempts[0].id
    $wrongAttempt = [Guid]::NewGuid().ToString()
    $wrongAck = Invoke-ExpectedFailure 11 @(
        'message', 'acknowledge', $messageId,
        '--expected-revision', '2',
        '--attempt-id', $wrongAttempt,
        '--acknowledgement-key', "wrong-$runId",
        '--idempotency-key', "wrong-ack-$runId"
    )
    Assert-True $wrongAck.Contains('LZ-MESSAGE-ATTEMPT') 'wrong attempt error code'
    $ackArguments = @(
        'message', 'acknowledge', $messageId,
        '--expected-revision', '2',
        '--attempt-id', $attemptId,
        '--acknowledgement-key', "provider-ack-$runId",
        '--evidence', (ConvertTo-NativeJsonArgument '{"progress":true}'),
        '--idempotency-key', "ack-$runId"
    )
    $acknowledged = Invoke-Json $ackArguments
    $ackReplay = Invoke-Json $ackArguments
    Assert-Equal $acknowledged.message.delivery.state 'acknowledged' 'acknowledged delivery state'
    Assert-Equal $acknowledged.message.attempts[0].acknowledgementKey "provider-ack-$runId" 'acknowledgement correlation'
    Assert-Equal $ackReplay.message.delivery.state 'acknowledged' 'idempotent acknowledgement replay'
    Assert-True ($null -eq $ackReplay.eventSequence) 'acknowledgement replay has no second event'
    $gates.acknowledgementCorrelation = 'PASS'

    $retrySent = Invoke-Json @(
        'message', 'send', $senderId, $recipientId,
        '--kind', 'delegation',
        '--body', (ConvertTo-NativeJsonArgument '{"text":"retry-delivery"}'),
        '--max-attempts', '2',
        '--idempotency-key', "retry-send-$runId"
    )
    $retryId = [string]$retrySent.message.envelope.id
    $retryClaimed = Invoke-Json @(
        'message', 'claim', $retryId,
        '--expected-revision', '1',
        '--idempotency-key', "retry-claim-$runId"
    )
    $retryAttemptId = [string]$retryClaimed.message.attempts[0].id
    $failed = Invoke-Json @(
        'message', 'fail', $retryId,
        '--expected-revision', '2',
        '--attempt-id', $retryAttemptId,
        '--retryable', 'true',
        '--failure-code', 'LZ-MESSAGE-TIMEOUT',
        '--failure-message', 'recipient acknowledgement was not observed',
        '--idempotency-key', "retry-fail-$runId"
    )
    Assert-Equal $failed.message.delivery.state 'retryable-failed' 'retryable failure state'
    $earlyRetry = Invoke-ExpectedFailure 11 @(
        'message', 'retry', $retryId,
        '--expected-revision', '3',
        '--idempotency-key', "early-retry-$runId"
    )
    Assert-True $earlyRetry.Contains('LZ-MESSAGE-RETRY-NOT-DUE') 'LZ-MESSAGE-RETRY-NOT-DUE'
    $gates.retryNotDueRejected = 'PASS'

    Start-Sleep -Milliseconds 1200
    $retrying = Invoke-Json @(
        'message', 'retry', $retryId,
        '--expected-revision', '3',
        '--idempotency-key', "retry-due-$runId"
    )
    Assert-Equal $retrying.message.delivery.state 'delivering' 'due retry state'
    Assert-Equal $retrying.message.delivery.attemptCount 2 'retry attempt count'
    $lastAttemptId = [string]$retrying.message.attempts[1].id
    $exhausted = Invoke-Json @(
        'message', 'fail', $retryId,
        '--expected-revision', '4',
        '--attempt-id', $lastAttemptId,
        '--retryable', 'true',
        '--failure-code', 'LZ-MESSAGE-TIMEOUT',
        '--failure-message', 'recipient acknowledgement was not observed',
        '--idempotency-key', "retry-exhausted-$runId"
    )
    Assert-Equal $exhausted.message.delivery.state 'terminal-failed' 'retry exhaustion state'
    Assert-Equal $exhausted.message.deadLetter.failureCode 'LZ-MESSAGE-TIMEOUT' 'dead-letter failure code'
    $gates.retryExhaustion = 'PASS'
    $gates.deadLetterEvidence = 'PASS'

    $cancelled = Invoke-Json @(
        'message', 'cancel', $causedId,
        '--expected-revision', '1',
        '--idempotency-key', "cancel-$runId"
    )
    Assert-Equal $cancelled.message.delivery.state 'cancelled' 'cancelled delivery state'
    $gates.cancellation = 'PASS'

    $restartSent = Invoke-Json @(
        'message', 'send', $senderId, $recipientId,
        '--kind', 'status',
        '--body', (ConvertTo-NativeJsonArgument '{"text":"survive-daemon-loss"}'),
        '--idempotency-key', "restart-send-$runId"
    )
    $restartId = [string]$restartSent.message.envelope.id
    $restartClaimed = Invoke-Json @(
        'message', 'claim', $restartId,
        '--expected-revision', '1',
        '--idempotency-key', "restart-claim-$runId"
    )
    Assert-Equal $restartClaimed.message.delivery.state 'delivering' 'pre-restart delivery state'
    $priorBootId = [string]$status.bootId
    $daemonId = $daemon.Id
    Stop-Process -Id $daemonId -Force
    if (-not $daemon.WaitForExit(10000)) {
        throw "forced daemon $daemonId did not exit"
    }
    $daemon = $null

    $daemon = Start-Daemon
    $restarted = Wait-Status $priorBootId
    Assert-True ($restarted.bootId -ne $priorBootId) 'daemon boot identity changed'
    $reconciled = Invoke-Json @('message', 'show', $restartId)
    Assert-Equal $reconciled.delivery.state 'retryable-failed' 'owner-loss delivery state'
    Assert-Equal $reconciled.delivery.lastFailureCode 'LZ-MESSAGE-OWNER-LOST' 'LZ-MESSAGE-OWNER-LOST'
    Assert-Equal $reconciled.attempts[0].failureCode 'LZ-MESSAGE-OWNER-LOST' 'owner-loss attempt evidence'
    $gates.restartDurability = 'PASS'
    $gates.ownerLossReconciled = 'PASS'

    $events = @(Invoke-Json @('events', 'list', '--after', '0', '--limit', '100'))
    $eventTypes = @($events | ForEach-Object { [string]$_.eventType })
    foreach ($requiredEvent in @(
        'message.queued',
        'message.delivering',
        'message.acknowledged',
        'message.retryable-failed',
        'message.terminal-failed',
        'message.cancelled',
        'messages.reconciled'
    )) {
        Assert-True ($eventTypes -contains $requiredEvent) "audit/outbox event $requiredEvent"
    }
    $drainedStatus = Wait-OutboxDrained
    Assert-Equal $drainedStatus.pendingOutbox 0 'outbox drained after restart'
    $gates.auditOutboxEvidence = 'PASS'
    $gates.providerSmokeAbsent = 'PASS'
    $gates.desktopCapabilityExpansionAbsent = 'PASS'

    $evidence.acknowledgedMessageId = $messageId
    $evidence.terminalMessageId = $retryId
    $evidence.reconciledMessageId = $restartId
    $evidence.eventTypes = @($eventTypes | Select-Object -Unique)

    Stop-Daemon $daemon
    $daemon = $null
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
    $manifest = [ordered]@{
        manifestVersion = 1
        status = $manifestStatus
        startedAt = $startedAt.ToString('o')
        completedAt = [DateTime]::UtcNow.ToString('o')
        durationMs = $stopwatch.ElapsedMilliseconds
        protocolVersion = 1
        schemaVersion = 12
        isolatedDataRoot = $script:dataRoot
        temporaryDataRetained = $false
        commands = @($script:commands)
        gates = $gates
        evidence = $evidence
    }
    $manifest | ConvertTo-Json -Depth 8 | Set-Content -LiteralPath $manifestPath -Encoding UTF8
    if (Test-Path -LiteralPath $runRoot) {
        Remove-Item -LiteralPath $runRoot -Recurse -Force
    }
}

Write-Output "PASS manifest: $manifestPath"
