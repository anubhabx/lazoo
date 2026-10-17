param(
    [switch]$SkipBuild,
    [string]$Provider = 'codex'
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

function Stop-Daemon([System.Diagnostics.Process]$Process) {
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

function Invoke-ContainmentProbe {
    $timer = [Diagnostics.Stopwatch]::StartNew()
    $previousPreference = $ErrorActionPreference
    try {
        $ErrorActionPreference = 'Continue'
        $captured = @(& $script:jobContainmentExe 2>&1)
        $exitCode = $LASTEXITCODE
    } finally {
        $ErrorActionPreference = $previousPreference
        $timer.Stop()
    }
    $script:commands.Add([ordered]@{
        command = 'lazoo-job-containment.exe'
        exitCode = $exitCode
        durationMs = $timer.ElapsedMilliseconds
    })
    Assert-Equal $exitCode 0 'creation-time containment probe exit code'
    return (($captured -join [Environment]::NewLine) | ConvertFrom-Json)
}

$repoRoot = (Resolve-Path (Join-Path $PSScriptRoot '..\..')).Path
$cargoExe = (Get-Command cargo -CommandType Application -ErrorAction Stop).Source
$releaseDir = Join-Path $repoRoot 'target\release'
$script:daemonExe = Join-Path $releaseDir 'lazood.exe'
$script:ctlExe = Join-Path $releaseDir 'lazooctl.exe'
$script:sessionHostExe = Join-Path $releaseDir 'lazoo-session-host.exe'
$script:jobContainmentExe = Join-Path $releaseDir 'lazoo-job-containment.exe'
$artifactDir = Join-Path $repoRoot 'artifacts\acceptance'
$manifestPath = Join-Path $artifactDir 'native-session-ownership-manifest.json'
$runId = [Guid]::NewGuid().ToString('N')
$tempBase = [IO.Path]::GetFullPath([IO.Path]::GetTempPath())
$runRoot = [IO.Path]::GetFullPath((Join-Path $tempBase "lazoo-native-session-$runId"))
if (-not $runRoot.StartsWith($tempBase, [StringComparison]::OrdinalIgnoreCase)) {
    throw 'acceptance run root is outside the system temporary directory'
}
$script:dataRoot = Join-Path $runRoot 'data'
$startedAt = [DateTime]::UtcNow
$stopwatch = [Diagnostics.Stopwatch]::StartNew()
$script:commands = [Collections.Generic.List[object]]::new()
$ownedHostPids = [Collections.Generic.List[int]]::new()
$gates = [ordered]@{}
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
    foreach ($binary in @($script:daemonExe, $script:ctlExe, $script:sessionHostExe, $script:jobContainmentExe)) {
        Assert-True (Test-Path -LiteralPath $binary) "binary exists: $binary"
    }
    $gates.releaseBuild = if ($SkipBuild) { 'SKIPPED_BY_CALLER' } else { 'PASS' }

    $containment = Invoke-ContainmentProbe
    Assert-Equal $containment.status 'PASS' 'containment probe status'
    Assert-True ([bool]$containment.creationTimeContainment) 'creationTimeContainment'
    Assert-True ([bool]$containment.completeTreeTerminated) 'completeTreeTerminated'
    $gates.creationTimeContainment = 'PASS'
    $gates.completeTreeTerminated = 'PASS'

    $daemon = Start-Daemon
    $status = Wait-Status
    Assert-Equal $status.protocolVersion 1 'protocol version'
    Assert-Equal $status.schemaVersion 19 'schemaVersion 19'
    Assert-Equal $status.journalMode 'wal' 'SQLite journal mode'
    $gates.schemaVersion = 'PASS'

    $created = Invoke-Json @(
        'employee', 'create', 'Session Acceptance',
        '--provider', $Provider,
        '--role', 'Acceptance employee',
        '--idempotency-key', "employee-create-$runId"
    )
    $employeeId = [string]$created.employee.id
    Assert-Equal $created.employee.configurationRevision 1 'employee revision'
    $roster = Invoke-Json @('roster', 'show')
    Assert-Equal $roster.entries[0].availability 'dormant' 'provider route readiness'
    Assert-True ([bool]$roster.entries[0].providerObservation.capabilities.session) 'provider session capability'
    $gates.providerReadyWithoutModelTurn = 'PASS'

    $warmArguments = @(
        'session', 'warm', $employeeId,
        '--expected-employee-revision', '1',
        '--idempotency-key', "session-warm-$runId"
    )
    $firstWarm = Invoke-Json $warmArguments
    $warmReplay = Invoke-Json $warmArguments
    Assert-Equal $firstWarm.session.id $warmReplay.session.id 'idempotent session identity'
    Assert-Equal $firstWarm.session.state 'idle' 'warmed session state'
    Assert-Equal $firstWarm.session.revision 2 'warmed session revision'
    Assert-True (-not [string]::IsNullOrWhiteSpace([string]$firstWarm.session.nativeSessionId)) 'native session identity'
    Assert-True ($null -ne $firstWarm.session.nativeHostPid) 'nativeHostPid'
    $sessionId = [string]$firstWarm.session.id
    $nativeHostPid = [int]$firstWarm.session.nativeHostPid
    $ownedHostPids.Add($nativeHostPid)
    $hostProcess = Get-ProcessRecord $nativeHostPid
    Assert-True ($null -ne $hostProcess) 'native session host process exists'
    Assert-Equal $hostProcess.ExecutablePath $script:sessionHostExe 'native session host executable'
    $nativeTree = @(Get-ProcessTreeRecords $nativeHostPid)
    Assert-True ($nativeTree.Count -ge 2) 'native session host has a provider descendant'
    Assert-True (Test-Path -LiteralPath (Join-Path $script:dataRoot "sessions\$sessionId")) 'protected session directory exists'
    $listed = @(Invoke-Json @('session', 'list'))
    Assert-Equal $listed.Count 1 'session list count'
    $shown = Invoke-Json @('session', 'show', $sessionId)
    Assert-Equal $shown.nativeHostPid $nativeHostPid 'shown nativeHostPid'
    $gates.idempotentWarm = 'PASS'
    $gates.nativeHostPid = 'PASS'

    $stale = Invoke-ExpectedFailure 10 @(
        'session', 'stop', $sessionId,
        '--expected-revision', '1',
        '--idempotency-key', "session-stale-stop-$runId"
    )
    Assert-True $stale.Contains('LZ-SESSION-STALE') 'stale session error code'
    Assert-True $stale.Contains('currentRevision') 'stale session current revision evidence'
    $gates.staleStopRejected = 'PASS'

    $stopArguments = @(
        'session', 'stop', $sessionId,
        '--expected-revision', '2',
        '--idempotency-key', "session-stop-$runId"
    )
    $stopTimer = [Diagnostics.Stopwatch]::StartNew()
    $stopped = Invoke-Json $stopArguments
    $stopTimer.Stop()
    $stopReplay = Invoke-Json $stopArguments
    Assert-True ($stopTimer.ElapsedMilliseconds -lt 12000) 'bounded session stop'
    Assert-Equal $stopped.session.state 'suspended' 'stopped session state'
    Assert-Equal $stopped.session.id $stopReplay.session.id 'idempotent stop session identity'
    Assert-True ($null -eq $stopped.session.nativeHostPid) 'stopped session clears native host PID'
    foreach ($record in $nativeTree) {
        Wait-ProcessGone ([int]$record.ProcessId) 10 'gracefully stopped native tree'
    }
    $suspendedRoster = Invoke-Json @('roster', 'show')
    Assert-Equal $suspendedRoster.entries[0].availability 'suspended' 'suspended roster availability'
    $gates.revisionCheckedBoundedStop = 'PASS'
    $gates.durableSuspendedRecord = 'PASS'

    $crashWarm = Invoke-Json @(
        'session', 'warm', $employeeId,
        '--expected-employee-revision', '1',
        '--idempotency-key', "session-crash-warm-$runId"
    )
    Assert-Equal $crashWarm.session.state 'idle' 'pre-crash session state'
    $crashSessionId = [string]$crashWarm.session.id
    $crashHostPid = [int]$crashWarm.session.nativeHostPid
    $ownedHostPids.Add($crashHostPid)
    Assert-True ($null -ne (Get-ProcessRecord $crashHostPid)) 'pre-crash native host process exists'
    $crashTree = @(Get-ProcessTreeRecords $crashHostPid)
    Assert-True ($crashTree.Count -ge 2) 'pre-crash native host has a provider descendant'
    $priorBootId = [string]$status.bootId
    $daemonId = $daemon.Id
    Stop-Process -Id $daemonId -Force
    if (-not $daemon.WaitForExit(10000)) {
        throw "forced daemon $daemonId did not exit"
    }
    $daemon = $null
    foreach ($record in $crashTree) {
        Wait-ProcessGone ([int]$record.ProcessId) 10 'daemon-loss native tree'
    }
    $gates.daemonLossTerminatesTree = 'PASS'
    $gates.liveProviderTreeTerminated = 'PASS'

    $daemon = Start-Daemon
    $restarted = Wait-Status $priorBootId
    Assert-True ($restarted.bootId -ne $priorBootId) 'daemon boot identity changed after forced loss'
    $reconciled = Invoke-Json @('session', 'show', $crashSessionId)
    Assert-Equal $reconciled.state 'failed' 'owner-loss session state'
    Assert-Equal $reconciled.failureCode 'LZ-SESSION-OWNER-LOST' 'LZ-SESSION-OWNER-LOST'
    Assert-True ($null -eq $reconciled.nativeHostPid) 'owner-loss clears native host PID'
    $reconciledRoster = Invoke-Json @('roster', 'show')
    Assert-Equal $reconciledRoster.entries[0].availability 'failed' 'owner-loss roster availability'
    $sessionsAfterRestart = @(Invoke-Json @('session', 'list'))
    Assert-Equal $sessionsAfterRestart.Count 2 'durable session history count'
    $gates.ownerLossReconciled = 'PASS'
    $gates.providerSmokeAbsent = 'PASS'

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
        Wait-Process -Id $daemon.Id -Timeout 10 -ErrorAction SilentlyContinue
    }
    foreach ($processId in @($ownedHostPids | Select-Object -Unique)) {
        $process = Get-ProcessRecord $processId
        if (($null -ne $process) -and ($process.ExecutablePath -eq $script:sessionHostExe)) {
            Stop-Process -Id $processId -Force
            Wait-Process -Id $processId -Timeout 10 -ErrorAction SilentlyContinue
        }
    }
    $manifest = [ordered]@{
        manifestVersion = 1
        status = $manifestStatus
        startedAt = $startedAt.ToString('o')
        completedAt = [DateTime]::UtcNow.ToString('o')
        durationMs = $stopwatch.ElapsedMilliseconds
        protocolVersion = 1
        schemaVersion = 19
        provider = $Provider
        isolatedDataRoot = $script:dataRoot
        temporaryDataRetained = $false
        commands = @($script:commands)
        gates = $gates
    }
    $manifest | ConvertTo-Json -Depth 8 | Set-Content -LiteralPath $manifestPath -Encoding UTF8
    if (Test-Path -LiteralPath $runRoot) {
        Remove-Item -LiteralPath $runRoot -Recurse -Force
    }
}

Write-Output "PASS manifest: $manifestPath"
