param(
    [switch]$SkipBuild,
    [int]$TerminalReadCycles = 8,
    [string]$OutputPath = ''
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
    $oldRoot = $env:LAZOO_DATA_ROOT
    $oldPreference = $ErrorActionPreference
    try {
        $env:LAZOO_DATA_ROOT = $script:dataRoot
        $ErrorActionPreference = 'Continue'
        $timer = [Diagnostics.Stopwatch]::StartNew()
        $captured = @(& $script:ctlExe --json @Arguments 2>&1)
        $exitCode = $LASTEXITCODE
        $timer.Stop()
        $script:commands.Add([ordered]@{
            command = "lazooctl --json $($Arguments -join ' ')"
            exitCode = $exitCode
            durationMs = [long]$timer.ElapsedMilliseconds
        })
        if ($exitCode -ne 0) {
            throw "lazooctl $($Arguments -join ' ') exited $exitCode with: $($captured -join [Environment]::NewLine)"
        }
        return (($captured -join [Environment]::NewLine) | ConvertFrom-Json)
    } finally {
        $ErrorActionPreference = $oldPreference
        $env:LAZOO_DATA_ROOT = $oldRoot
    }
}

function Wait-Status([int]$TimeoutSeconds = 20) {
    $deadline = [DateTime]::UtcNow.AddSeconds($TimeoutSeconds)
    $lastError = ''
    while ([DateTime]::UtcNow -lt $deadline) {
        try { return Invoke-Json @('status') } catch { $lastError = $_.Exception.Message }
        Start-Sleep -Milliseconds 100
    }
    throw "daemon did not become ready: $lastError"
}

function Start-Keeper([string]$DataRoot, [string]$DaemonPath, [string]$LogPrefix) {
    $info = [Diagnostics.ProcessStartInfo]::new()
    $info.FileName = $script:keeperExe
    $info.UseShellExecute = $false
    $info.CreateNoWindow = $true
    $info.RedirectStandardOutput = $true
    $info.RedirectStandardError = $true
    $info.EnvironmentVariables['LAZOO_DATA_ROOT'] = $DataRoot
    $info.EnvironmentVariables['LAZOO_DAEMON_PATH'] = $DaemonPath
    $process = [Diagnostics.Process]::Start($info)
    $process | Add-Member -NotePropertyName LazooLogPrefix -NotePropertyValue $LogPrefix
    $process | Add-Member -NotePropertyName LazooStartedAt -NotePropertyValue $process.StartTime.ToUniversalTime()
    return $process
}

function Stop-ExactProcess([Diagnostics.Process]$Process) {
    if ($null -eq $Process) { return }
    $current = Get-Process -Id $Process.Id -ErrorAction SilentlyContinue
    if ($null -eq $current) { return }
    if ($current.StartTime.ToUniversalTime() -ne $Process.LazooStartedAt) {
        throw "refusing to stop reused PID $($Process.Id)"
    }
    Stop-Process -Id $Process.Id -Force
    $null = $current.WaitForExit(5000)
}

function Wait-Exit([Diagnostics.Process]$Process, [int]$TimeoutSeconds, [string]$Label) {
    if (-not $Process.WaitForExit($TimeoutSeconds * 1000)) { throw "$Label did not exit within $TimeoutSeconds seconds" }
    Set-Content -LiteralPath "$($Process.LazooLogPrefix).stdout.log" -Value $Process.StandardOutput.ReadToEnd() -Encoding UTF8
    Set-Content -LiteralPath "$($Process.LazooLogPrefix).stderr.log" -Value $Process.StandardError.ReadToEnd() -Encoding UTF8
    return [int]$Process.ExitCode
}

function Get-ExactChild([Diagnostics.Process]$Parent, [string]$ExpectedPath, [int]$TimeoutSeconds = 10) {
    $deadline = [DateTime]::UtcNow.AddSeconds($TimeoutSeconds)
    while ([DateTime]::UtcNow -lt $deadline) {
        $rows = @(Get-CimInstance Win32_Process -Filter "ParentProcessId = $($Parent.Id)")
        foreach ($row in $rows) {
            if ($row.ExecutablePath -eq $ExpectedPath) {
                $child = Get-Process -Id $row.ProcessId -ErrorAction SilentlyContinue
                if ($null -ne $child) {
                    $child | Add-Member -NotePropertyName LazooStartedAt -NotePropertyValue $child.StartTime.ToUniversalTime()
                    return $child
                }
            }
        }
        Start-Sleep -Milliseconds 100
    }
    throw "keeper did not start expected daemon $ExpectedPath"
}

if ($TerminalReadCycles -lt 2 -or $TerminalReadCycles -gt 20) {
    throw 'TerminalReadCycles must be between 2 and 20'
}

$repoRoot = (Resolve-Path (Join-Path $PSScriptRoot '..\..')).Path
$cargoExe = (Get-Command cargo -CommandType Application -ErrorAction Stop).Source
$releaseDir = Join-Path $repoRoot 'target\release'
$script:keeperExe = Join-Path $releaseDir 'lazoo-keeper.exe'
$script:daemonExe = Join-Path $releaseDir 'lazood.exe'
$script:ctlExe = Join-Path $releaseDir 'lazooctl.exe'
$runId = [Guid]::NewGuid().ToString('N')
$runRoot = Join-Path ([IO.Path]::GetTempPath()) "lazoo-core-services-$runId"
$script:dataRoot = Join-Path $runRoot 'data'
$artifactDir = Join-Path $repoRoot 'artifacts\acceptance'
if ([string]::IsNullOrWhiteSpace($OutputPath)) {
    $OutputPath = Join-Path $artifactDir 'core-services-manifest.json'
}
$manifestPath = [IO.Path]::GetFullPath($OutputPath)
$startedAt = [DateTime]::UtcNow
$timer = [Diagnostics.Stopwatch]::StartNew()
$script:commands = [Collections.Generic.List[object]]::new()
$checks = [ordered]@{}
$measurements = [ordered]@{}
$keeper = $null
$daemon = $null
$manifestStatus = 'FAIL'

New-Item -ItemType Directory -Force -Path $script:dataRoot, $artifactDir | Out-Null

try {
    if (-not $SkipBuild) {
        $buildTimer = [Diagnostics.Stopwatch]::StartNew()
        & $cargoExe build -p lazoo-keeper -p lazood -p lazooctl --release --locked
        $buildExit = $LASTEXITCODE
        $buildTimer.Stop()
        $script:commands.Add([ordered]@{ command = 'cargo build -p lazoo-keeper -p lazood -p lazooctl --release --locked'; exitCode = $buildExit; durationMs = [long]$buildTimer.ElapsedMilliseconds })
        Assert-Equal $buildExit 0 'release build'
    }
    foreach ($binary in @($script:keeperExe, $script:daemonExe, $script:ctlExe)) {
        Assert-True (Test-Path -LiteralPath $binary) "binary exists: $binary"
    }
    $checks.releaseBuild = if ($SkipBuild) { 'SKIPPED_BY_CALLER' } else { 'PASS' }

    $keeper = Start-Keeper $script:dataRoot $script:daemonExe (Join-Path $runRoot 'keeper')
    $status = Wait-Status
    Assert-True ($null -ne $status.bootId) 'daemon status has boot id'
    $daemon = Get-ExactChild $keeper $script:daemonExe

    $budget = Invoke-Json @('system', 'budget')
    Assert-Equal ([bool]$budget.enforced) $true 'infrastructure CPU cap is enforced'
    Assert-Equal ([int]$budget.hardCapPercent) 20 'infrastructure CPU hard cap'
    Assert-Equal ([bool]$budget.providerTreesIncluded) $false 'provider trees are excluded from shared infrastructure accounting'
    Assert-Equal ([bool]$budget.desktopIncluded) $false 'desktop is not claimed in this daemon-only scenario'
    Assert-True ([int]$budget.activeProcesses -ge 2) 'shared infrastructure job accounts for keeper and daemon'
    Assert-True ([int]$budget.totalProcesses -ge [int]$budget.activeProcesses) 'job accounting process totals are coherent'
    $checks.infrastructureBudget = 'PASS'

    $policy = Invoke-Json @('memory', 'policy-show', '--global')
    Assert-Equal ([int]$policy.revision) 1 'default memory policy revision'
    $worker = Invoke-Json @('memory-worker', 'show')
    Assert-True ($null -eq $worker.settings) 'memory worker is disabled until explicitly configured'
    $queue = Invoke-Json @('memory', 'queue-status', '--limit', '20')
    Assert-Equal @($queue.jobs).Count 0 'new data root has no memory summary jobs'
    Assert-Equal ([int]$queue.archiveErrorCount) 0 'new data root has no memory archive errors'
    $checks.memoryDefaultsAndQueue = 'PASS'

    $open = Invoke-Json @(
        'terminal', 'open', "$env:SystemRoot\System32\cmd.exe",
        '--arg', '/d', '--arg', '/c',
        '--arg', 'for /L %i in (1,1,48) do @echo core-services-%i & ping -n 7 127.0.0.1 >nul',
        '--cwd', $runRoot, '--columns', '100', '--rows', '30',
        '--idempotency-key', "core-services-terminal-$runId"
    )
    Assert-True ($null -ne $open.id) 'terminal open returns an id'
    Assert-True ([bool]$open.live) 'terminal is live after open'
    $resized = Invoke-Json @('terminal', 'resize', $open.id, '--columns', '120', '--rows', '40', '--expected-revision', $open.revision, '--expected-boot-id', $open.bootId)
    Assert-Equal ([int]$resized.columns) 120 'terminal resize columns'
    Assert-Equal ([int]$resized.rows) 40 'terminal resize rows'

    $nonEmptyReads = 0
    $after = 0
    for ($cycle = 0; $cycle -lt $TerminalReadCycles; $cycle += 1) {
        $read = Invoke-Json @('terminal', 'read', $open.id, '--expected-boot-id', $open.bootId, '--after', $after, '--limit', '4096')
        Assert-True ($null -ne $read.output) "terminal read $cycle has output metadata"
        Assert-True ([uint64]$read.output.end -ge [uint64]$read.output.start) "terminal read $cycle has valid offsets"
        if (@($read.bytes).Count -gt 0) { $nonEmptyReads += 1 }
        $after = [uint64]$read.output.end
        Start-Sleep -Milliseconds 80
    }
    Assert-True ($nonEmptyReads -ge 1) 'bounded terminal read load produced output'
    $stopped = Invoke-Json @('terminal', 'stop', $open.id, '--expected-revision', $resized.revision, '--expected-boot-id', $open.bootId)
    Assert-Equal ([bool]$stopped.live) $false 'terminal stop ends the child session'
    $checks.terminalControlAndLoad = 'PASS'

    $daemon.Refresh()
    $measurements.daemon = [ordered]@{
        pid = [int]$daemon.Id
        cpuMillis = [long]$daemon.TotalProcessorTime.TotalMilliseconds
        workingSetBytes = [long]$daemon.WorkingSet64
        peakWorkingSetBytes = [long]$daemon.PeakWorkingSet64
        handleCount = [int]$daemon.HandleCount
        terminalReadCycles = $TerminalReadCycles
        nonEmptyReads = $nonEmptyReads
    }
    $checks.daemonResourcesSampled = 'PASS'

    Invoke-Json @('system', 'shutdown') | Out-Null
    Assert-Equal (Wait-Exit $keeper 15 'keeper') 0 'planned keeper exit code'
    $checks.plannedShutdown = 'PASS'
    $manifestStatus = 'PASS'
} finally {
    if ($null -ne $daemon) { Stop-ExactProcess $daemon }
    if ($null -ne $keeper) { Stop-ExactProcess $keeper }
    $timer.Stop()
    $manifest = [ordered]@{
        status = $manifestStatus
        mode = 'real-binaries-disposable-data-root-no-provider'
        startedAt = $startedAt.ToString('o')
        durationMs = [long]$timer.ElapsedMilliseconds
        dataRoot = $script:dataRoot
        checks = $checks
        measurements = $measurements
        commands = @($script:commands)
        boundaries = [ordered]@{
            externalModelUse = 'NOT_RUN'
            userConfiguration = 'NOT_READ_OR_WRITTEN'
            providerProcess = 'NOT_STARTED'
            desktop = 'NOT_RUNNING'
            fiveMinuteCpuWindow = 'NOT_COVERED'
            desktopInfrastructureCap = 'NOT_COVERED'
        }
    }
    $manifest | ConvertTo-Json -Depth 8 | Set-Content -LiteralPath $manifestPath -Encoding UTF8
}

Write-Output "PASS manifest: $manifestPath"
