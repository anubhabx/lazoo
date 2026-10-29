param(
    [switch]$SkipBuild,
    [int]$WindowSeconds = 30
)

$ErrorActionPreference = 'Stop'
Set-StrictMode -Version Latest

function Assert-Equal($Actual, $Expected, [string]$Label) {
    if ($Actual -ne $Expected) { throw "$Label expected '$Expected' but found '$Actual'" }
}

function Assert-True([bool]$Condition, [string]$Label) {
    if (-not $Condition) { throw "$Label expected true" }
}

function Invoke-Json([string[]]$Arguments) {
    $previousRoot = $env:LAZOO_DATA_ROOT
    $previousPreference = $ErrorActionPreference
    try {
        $env:LAZOO_DATA_ROOT = $script:dataRoot
        $ErrorActionPreference = 'Continue'
        $commandTimer = [Diagnostics.Stopwatch]::StartNew()
        $captured = @(& $script:ctlExe --json @Arguments 2>&1)
        $exitCode = $LASTEXITCODE
        $commandTimer.Stop()
        $script:commands.Add([ordered]@{
            command = "lazooctl --json $($Arguments -join ' ')"
            exitCode = $exitCode
            durationMs = $commandTimer.ElapsedMilliseconds
        })
        if ($exitCode -ne 0) {
            throw "lazooctl $($Arguments -join ' ') exited $exitCode with: $($captured -join [Environment]::NewLine)"
        }
        return (($captured -join [Environment]::NewLine) | ConvertFrom-Json)
    } finally {
        $ErrorActionPreference = $previousPreference
        $env:LAZOO_DATA_ROOT = $previousRoot
    }
}

function Wait-Status([int]$TimeoutSeconds = 20) {
    $deadline = [DateTime]::UtcNow.AddSeconds($TimeoutSeconds)
    $lastError = ''
    while ([DateTime]::UtcNow -lt $deadline) {
        try { return (Invoke-Json @('status')) } catch { $lastError = $_.Exception.Message }
        Start-Sleep -Milliseconds 100
    }
    throw "daemon did not become ready: $lastError"
}

function Start-Keeper([string]$DataRoot, [string]$LogPrefix) {
    $startInfo = [Diagnostics.ProcessStartInfo]::new()
    $startInfo.FileName = $script:keeperExe
    $startInfo.UseShellExecute = $false
    $startInfo.CreateNoWindow = $true
    $startInfo.RedirectStandardOutput = $true
    $startInfo.RedirectStandardError = $true
    $startInfo.EnvironmentVariables['LAZOO_DATA_ROOT'] = $DataRoot
    $startInfo.EnvironmentVariables.Remove('LAZOO_DAEMON_PATH')
    $process = [Diagnostics.Process]::Start($startInfo)
    $process | Add-Member -NotePropertyName LazooLogPrefix -NotePropertyValue $LogPrefix
    return $process
}

function Wait-Exit([System.Diagnostics.Process]$Process, [int]$TimeoutSeconds, [string]$Label) {
    if (-not $Process.WaitForExit($TimeoutSeconds * 1000)) { throw "$Label did not exit within $TimeoutSeconds seconds" }
    $stdout = $Process.StandardOutput.ReadToEnd()
    $stderr = $Process.StandardError.ReadToEnd()
    Set-Content -LiteralPath "$($Process.LazooLogPrefix).stdout.log" -Value $stdout -Encoding UTF8
    Set-Content -LiteralPath "$($Process.LazooLogPrefix).stderr.log" -Value $stderr -Encoding UTF8
    [int]$exitCode = $Process.ExitCode
    return $exitCode
}

# One sample of "Lazoo itself" (brief section 20): the daemon through its own accounting method,
# the keeper through the process table. Native provider trees are not part of it.
function Get-Sample([System.Diagnostics.Process]$Keeper) {
    $daemon = Invoke-Json @('system', 'resources')
    $Keeper.Refresh()
    $keeperCpuMillis = [long]$Keeper.TotalProcessorTime.TotalMilliseconds
    return [ordered]@{
        sampledAt = [DateTime]::UtcNow
        daemonPid = [int]$daemon.pid
        daemonCpuMillis = [long]$daemon.cpuMillis
        daemonWorkingSetBytes = [long]$daemon.workingSetBytes
        daemonPeakWorkingSetBytes = [long]$daemon.peakWorkingSetBytes
        daemonHandleCount = [int]$daemon.handleCount
        keeperCpuMillis = $keeperCpuMillis
        keeperWorkingSetBytes = [long]$Keeper.WorkingSet64
    }
}

# Share of total machine capacity used by keeper plus daemon between two samples.
function Get-CpuShare($Before, $After) {
    $wallMillis = ($After.sampledAt - $Before.sampledAt).TotalMilliseconds
    $cpuMillis = ($After.daemonCpuMillis - $Before.daemonCpuMillis) + ($After.keeperCpuMillis - $Before.keeperCpuMillis)
    return [double]$cpuMillis / ([double]$wallMillis * [Environment]::ProcessorCount)
}

$repoRoot = (Resolve-Path (Join-Path $PSScriptRoot '..\..')).Path
$cargoExe = (Get-Command cargo -CommandType Application -ErrorAction Stop).Source
$releaseDir = Join-Path $repoRoot 'target\release'
$script:keeperExe = Join-Path $releaseDir 'lazoo-keeper.exe'
$script:daemonExe = Join-Path $releaseDir 'lazood.exe'
$script:ctlExe = Join-Path $releaseDir 'lazooctl.exe'
$artifactDir = Join-Path $repoRoot 'artifacts\acceptance'
$runId = [Guid]::NewGuid().ToString('N')
$runRoot = Join-Path ([System.IO.Path]::GetTempPath()) "lazoo-budget-$runId"
$script:dataRoot = Join-Path $runRoot 'data'
$dataRoot = $script:dataRoot
$projectRoot = Join-Path $runRoot 'repository'
$manifestPath = Join-Path $artifactDir 'resource-budget-manifest.json'
$startedAt = [DateTime]::UtcNow
$stopwatch = [Diagnostics.Stopwatch]::StartNew()
$script:commands = [Collections.Generic.List[object]]::new()
$gates = [ordered]@{}
$measurements = [ordered]@{}
$keeper = $null
$manifestStatus = 'FAIL'

# Section 20.1 targets, as shares of total machine capacity and bytes.
$idleCpuCeiling = 0.01
$routingCpuCeiling = 0.05
$stormCpuCeiling = 0.20
$idleMemoryCeilingBytes = 350MB
$activeMemoryCeilingBytes = 750MB

function ConvertTo-NativeJsonArgument([string]$Value) {
    if ($PSVersionTable.PSVersion.Major -lt 7) {
        return $Value.Replace('"', '\"')
    }
    return $Value
}

New-Item -ItemType Directory -Force -Path $artifactDir, $dataRoot, $projectRoot | Out-Null

try {
    if (-not $SkipBuild) {
        $commandTimer = [Diagnostics.Stopwatch]::StartNew()
        & $cargoExe build --workspace --release --locked
        $buildExit = $LASTEXITCODE
        $commandTimer.Stop()
        $script:commands.Add([ordered]@{ command = 'cargo build --workspace --release --locked'; exitCode = $buildExit; durationMs = $commandTimer.ElapsedMilliseconds })
        Assert-Equal $buildExit 0 'locked release build'
    }
    foreach ($binary in @($script:keeperExe, $script:daemonExe, $script:ctlExe)) {
        Assert-True (Test-Path -LiteralPath $binary) "binary exists: $binary"
    }
    $gates.releaseBuild = if ($SkipBuild) { 'SKIPPED_BY_CALLER' } else { 'PASS' }

    & git init --quiet $projectRoot
    Assert-Equal $LASTEXITCODE 0 'temporary Git repository initialization'

    $keeper = Start-Keeper $dataRoot (Join-Path $runRoot 'keeper')
    $status = Wait-Status
    Assert-Equal $status.schemaVersion 20 'schema version'
    $gates.schemaVersion = 'PASS'

    # Let startup settle before the idle window, so the measurement is steady state.
    Start-Sleep -Seconds 3
    $idleBefore = Get-Sample $keeper
    Assert-True ($idleBefore.daemonPid -gt 0) 'daemon reports its own PID'
    $daemonProcess = Get-Process -Id $idleBefore.daemonPid
    Assert-Equal $daemonProcess.Path $script:daemonExe 'system.resources names the real daemon'
    Start-Sleep -Seconds $WindowSeconds
    $idleAfter = Get-Sample $keeper
    $idleShare = Get-CpuShare $idleBefore $idleAfter
    $idleMemory = $idleAfter.daemonWorkingSetBytes + $idleAfter.keeperWorkingSetBytes
    $measurements.idle = [ordered]@{
        windowSeconds = $WindowSeconds
        cpuShare = $idleShare
        daemonCpuMillis = $idleAfter.daemonCpuMillis - $idleBefore.daemonCpuMillis
        keeperCpuMillis = $idleAfter.keeperCpuMillis - $idleBefore.keeperCpuMillis
        daemonWorkingSetBytes = $idleAfter.daemonWorkingSetBytes
        keeperWorkingSetBytes = $idleAfter.keeperWorkingSetBytes
        lazooWorkingSetBytes = $idleMemory
        daemonHandleCount = $idleAfter.daemonHandleCount
    }
    Assert-True ($idleShare -lt $idleCpuCeiling) "idle CPU share $idleShare under $idleCpuCeiling"
    $gates.idleCpuUnderOnePercent = 'PASS'
    Assert-True ($idleMemory -lt $idleMemoryCeilingBytes) "idle working set $idleMemory under $idleMemoryCeilingBytes"
    $gates.idleMemoryUnder350MiB = 'PASS'

    # Normal routing: a steady stream of control-plane reads and one durable write for the
    # window, measured the same way. No provider process is started.
    $routingBefore = Get-Sample $keeper
    $routingCalls = 0
    $registration = Invoke-Json @('project', 'register', $projectRoot, '--idempotency-key', "budget-$runId")
    Assert-True ($null -ne $registration.project.id) 'project registered during the routing window'
    $routingDeadline = [DateTime]::UtcNow.AddSeconds($WindowSeconds)
    while ([DateTime]::UtcNow -lt $routingDeadline) {
        Invoke-Json @('status') | Out-Null
        Invoke-Json @('project', 'list') | Out-Null
        Invoke-Json @('events', 'list', '--after', '0', '--limit', '100') | Out-Null
        Invoke-Json @('inbox', 'counts') | Out-Null
        Invoke-Json @('roster', 'show') | Out-Null
        $routingCalls += 5
    }
    $routingAfter = Get-Sample $keeper
    $routingShare = Get-CpuShare $routingBefore $routingAfter
    $measurements.routing = [ordered]@{
        windowSeconds = $WindowSeconds
        calls = $routingCalls
        cpuShare = $routingShare
        daemonCpuMillis = $routingAfter.daemonCpuMillis - $routingBefore.daemonCpuMillis
        keeperCpuMillis = $routingAfter.keeperCpuMillis - $routingBefore.keeperCpuMillis
        daemonWorkingSetBytes = $routingAfter.daemonWorkingSetBytes
        daemonPeakWorkingSetBytes = $routingAfter.daemonPeakWorkingSetBytes
        lazooWorkingSetBytes = $routingAfter.daemonWorkingSetBytes + $routingAfter.keeperWorkingSetBytes
    }
    Assert-True ($routingCalls -ge 5) 'routing window issued calls'
    Assert-True ($routingShare -lt $routingCpuCeiling) "routing CPU share $routingShare under $routingCpuCeiling"
    $gates.routingCpuUnderFivePercent = 'PASS'
    Assert-True (($routingAfter.daemonWorkingSetBytes + $routingAfter.keeperWorkingSetBytes) -lt $idleMemoryCeilingBytes) 'routing working set under the idle ceiling'
    $gates.routingMemoryUnder350MiB = 'PASS'

    # Failure storm (section 20.2): one-attempt messages sent, claimed, and failed as fast as
    # the CLI can for the window, so every cycle dead-letters, raises and folds an incident,
    # promotes it by the section 14.2 threshold, escalates it to the user, and the timed sweep
    # announces it once. Measured the same way against the hard ceiling and the active-core
    # memory target. Nothing is enforced.
    $stormSender = Invoke-Json @(
        'employee', 'create', 'Storm',
        '--provider', 'codex',
        '--role', 'Lead',
        '--idempotency-key', "storm-sender-$runId"
    )
    $stormRecipient = Invoke-Json @(
        'employee', 'create', 'Target',
        '--provider', 'codex',
        '--role', 'Worker',
        '--idempotency-key', "storm-recipient-$runId"
    )
    $stormSenderId = [string]$stormSender.employee.id
    $stormRecipientId = [string]$stormRecipient.employee.id
    $stormBody = ConvertTo-NativeJsonArgument '{"text":"failure-storm"}'
    $stormBefore = Get-Sample $keeper
    $stormCycles = 0
    $stormDeadline = [DateTime]::UtcNow.AddSeconds($WindowSeconds)
    while ([DateTime]::UtcNow -lt $stormDeadline) {
        $sent = Invoke-Json @(
            'message', 'send', $stormSenderId, $stormRecipientId,
            '--kind', 'peer',
            '--body', $stormBody,
            '--max-attempts', '1',
            '--idempotency-key', "storm-send-$runId-$stormCycles"
        )
        $stormMessageId = [string]$sent.message.envelope.id
        $claimed = Invoke-Json @(
            'message', 'claim', $stormMessageId,
            '--expected-revision', '1',
            '--idempotency-key', "storm-claim-$runId-$stormCycles"
        )
        $stormAttemptId = [string]$claimed.message.attempts[0].id
        $failed = Invoke-Json @(
            'message', 'fail', $stormMessageId,
            '--expected-revision', '2',
            '--attempt-id', $stormAttemptId,
            '--retryable', 'true',
            '--failure-code', 'LZ-MESSAGE-TIMEOUT',
            '--failure-message', 'failure storm: acknowledgement was not observed',
            '--idempotency-key', "storm-fail-$runId-$stormCycles"
        )
        if ($stormCycles -eq 0) {
            Assert-Equal $failed.message.delivery.state 'terminal-failed' 'a one-attempt failure dead-letters'
        }
        $stormCycles += 1
    }
    $stormAfter = Get-Sample $keeper
    $stormShare = Get-CpuShare $stormBefore $stormAfter
    $stormMemory = $stormAfter.daemonWorkingSetBytes + $stormAfter.keeperWorkingSetBytes
    $stormIncidents = @(Invoke-Json @('incident', 'list', '--limit', '100'))
    $stormOccurrences = 0
    foreach ($incident in $stormIncidents) {
        if ([int]$incident.occurrences -gt $stormOccurrences) { $stormOccurrences = [int]$incident.occurrences }
    }
    $measurements.failureStorm = [ordered]@{
        windowSeconds = $WindowSeconds
        cycles = $stormCycles
        deadLetters = $stormCycles
        incidents = $stormIncidents.Count
        foldedOccurrences = $stormOccurrences
        cpuShare = $stormShare
        daemonCpuMillis = $stormAfter.daemonCpuMillis - $stormBefore.daemonCpuMillis
        keeperCpuMillis = $stormAfter.keeperCpuMillis - $stormBefore.keeperCpuMillis
        daemonWorkingSetBytes = $stormAfter.daemonWorkingSetBytes
        daemonPeakWorkingSetBytes = $stormAfter.daemonPeakWorkingSetBytes
        lazooWorkingSetBytes = $stormMemory
    }
    Assert-True ($stormCycles -ge 5) 'failure storm drove cycles'
    Assert-True ($stormIncidents.Count -ge 1) 'the storm raised an incident'
    Assert-True ($stormOccurrences -ge 3) 'the storm folded into a promoted incident'
    Assert-True ($stormShare -lt $stormCpuCeiling) "failure storm CPU share $stormShare under $stormCpuCeiling"
    $gates.failureStormCpuUnderTwentyPercent = 'PASS'
    Assert-True ($stormMemory -lt $activeMemoryCeilingBytes) "failure storm working set $stormMemory under $activeMemoryCeilingBytes"
    $gates.failureStormMemoryUnder750MiB = 'PASS'

    $resources = Invoke-Json @('system', 'resources')
    Assert-Equal ([int]$resources.liveSessions) 0 'no provider session was started'
    $gates.noProviderProcess = 'PASS'

    Invoke-Json @('system', 'shutdown') | Out-Null
    Assert-Equal (Wait-Exit $keeper 15 'keeper') 0 'planned keeper exit code'
    $gates.plannedShutdown = 'PASS'
    $manifestStatus = 'PASS'
} finally {
    if ($null -ne $keeper -and -not $keeper.HasExited) {
        $ownedDaemon = Get-CimInstance Win32_Process -Filter "Name = 'lazood.exe'" |
            Where-Object { $_.ParentProcessId -eq $keeper.Id }
        foreach ($process in $ownedDaemon) { Stop-Process -Id $process.ProcessId -Force }
        Stop-Process -Id $keeper.Id -Force
    }
    $stopwatch.Stop()
    $manifest = [ordered]@{
        status = $manifestStatus
        schemaVersion = 20
        mode = 'real-binaries-no-provider'
        startedAt = $startedAt.ToString('o')
        durationMs = [long]$stopwatch.ElapsedMilliseconds
        processorCount = [Environment]::ProcessorCount
        targets = [ordered]@{
            idleCpuShare = $idleCpuCeiling
            routingCpuShare = $routingCpuCeiling
            failureStormCpuShare = $stormCpuCeiling
            idleWorkingSetBytes = [long]$idleMemoryCeilingBytes
            activeWorkingSetBytes = [long]$activeMemoryCeilingBytes
            windowSeconds = $WindowSeconds
        }
        measurements = $measurements
        gates = $gates
        commands = @($script:commands)
        boundaries = [ordered]@{
            providerSmoke = 'NOT_RUN'
            externalModelUse = 'NOT_RUN'
            desktopUi = 'NOT_RUNNING'
            measured = 'KEEPER_AND_DAEMON'
            notMeasured = 'DESKTOP_CLI_MEMORY_WORKERS_VOICE'
            scenarios = 'IDLE_ROUTING_AND_FAILURE_STORM'
            enforcement = 'NONE'
        }
    }
    $manifest | ConvertTo-Json -Depth 8 | Set-Content -LiteralPath $manifestPath -Encoding UTF8
}

Write-Output "PASS manifest: $manifestPath"
