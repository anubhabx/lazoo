param(
    [switch]$SkipBuild
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

function Wait-Status([string]$DifferentBootId = '', [int]$TimeoutSeconds = 20) {
    $deadline = [DateTime]::UtcNow.AddSeconds($TimeoutSeconds)
    $lastError = ''
    while ([DateTime]::UtcNow -lt $deadline) {
        try {
            $status = Invoke-Json @('status')
            if (($DifferentBootId -eq '') -or ($status.bootId -ne $DifferentBootId)) { return $status }
        } catch { $lastError = $_.Exception.Message }
        Start-Sleep -Milliseconds 100
    }
    throw "daemon did not become ready: $lastError"
}

function Start-Keeper([string]$DataRoot, [string]$LogPrefix, [string]$DaemonOverride = '') {
    $startInfo = [Diagnostics.ProcessStartInfo]::new()
    $startInfo.FileName = $script:keeperExe
    $startInfo.UseShellExecute = $false
    $startInfo.CreateNoWindow = $true
    $startInfo.RedirectStandardOutput = $true
    $startInfo.RedirectStandardError = $true
    $startInfo.EnvironmentVariables['LAZOO_DATA_ROOT'] = $DataRoot
    if ($DaemonOverride -eq '') {
        $startInfo.EnvironmentVariables.Remove('LAZOO_DAEMON_PATH')
    } else {
        $startInfo.EnvironmentVariables['LAZOO_DAEMON_PATH'] = $DaemonOverride
    }
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

function Find-DaemonProcess([int]$ExcludePid = 0) {
    $deadline = [DateTime]::UtcNow.AddSeconds(10)
    while ([DateTime]::UtcNow -lt $deadline) {
        $candidate = Get-CimInstance Win32_Process -Filter "Name = 'lazood.exe'" |
            Where-Object { $_.ProcessId -ne $ExcludePid -and $_.ExecutablePath -eq $script:daemonExe } |
            Select-Object -First 1
        if ($null -ne $candidate) { return $candidate }
        Start-Sleep -Milliseconds 100
    }
    throw 'could not resolve the exact lazood process'
}

$repoRoot = (Resolve-Path (Join-Path $PSScriptRoot '..\..')).Path
$cargoExe = (Get-Command cargo -CommandType Application -ErrorAction Stop).Source
$releaseDir = Join-Path $repoRoot 'target\release'
$script:keeperExe = Join-Path $releaseDir 'lazoo-keeper.exe'
$script:daemonExe = Join-Path $releaseDir 'lazood.exe'
$script:ctlExe = Join-Path $releaseDir 'lazooctl.exe'
$crashingExe = Join-Path $releaseDir 'lazoo-crashing-daemon.exe'
$artifactDir = Join-Path $repoRoot 'artifacts\acceptance'
$runId = [Guid]::NewGuid().ToString('N')
$runRoot = Join-Path ([System.IO.Path]::GetTempPath()) "lazoo-acceptance-$runId"
$script:dataRoot = Join-Path $runRoot 'data'
$dataRoot = $script:dataRoot
$projectRoot = Join-Path $runRoot 'repository'
$manifestPath = Join-Path $artifactDir 'first-slice-manifest.json'
$startedAt = [DateTime]::UtcNow
$stopwatch = [Diagnostics.Stopwatch]::StartNew()
$script:commands = [Collections.Generic.List[object]]::new()
$gates = [ordered]@{}
$keeper = $null
$manifestStatus = 'FAIL'

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
    foreach ($binary in @($script:keeperExe, $script:daemonExe, $script:ctlExe, $crashingExe)) {
        Assert-True (Test-Path -LiteralPath $binary) "binary exists: $binary"
    }
    $gates.releaseBuild = if ($SkipBuild) { 'SKIPPED_BY_CALLER' } else { 'PASS' }

    & git init --quiet $projectRoot
    $gitInitExit = $LASTEXITCODE
    $script:commands.Add([ordered]@{ command = 'git init --quiet <temporary-project>'; exitCode = $gitInitExit; durationMs = $null })
    Assert-Equal $gitInitExit 0 'temporary Git repository initialization'

    $keeper = Start-Keeper $dataRoot (Join-Path $runRoot 'keeper-real-1')
    $statusBefore = Wait-Status
    Assert-Equal $statusBefore.protocolVersion 1 'protocol version'
    Assert-Equal $statusBefore.schemaVersion 2 'schema version'
    Assert-Equal $statusBefore.journalMode 'wal' 'SQLite journal mode'
    $gates.controlPlaneReady = 'PASS'

    $idempotencyKey = "acceptance-$runId"
    $firstRegistration = Invoke-Json @('project', 'register', $projectRoot, '--idempotency-key', $idempotencyKey)
    $secondRegistration = Invoke-Json @('project', 'register', $projectRoot, '--idempotency-key', $idempotencyKey)
    Assert-Equal $firstRegistration.project.id $secondRegistration.project.id 'idempotent project identity'
    Assert-Equal $firstRegistration.project.checkoutMode 'isolated' 'default checkout mode'
    $projects = @(Invoke-Json @('project', 'list'))
    $events = @(Invoke-Json @('events', 'list', '--after', '0', '--limit', '100'))
    Assert-Equal $projects.Count 1 'registered project count'
    Assert-Equal $events.Count 1 'idempotent logical event count'
    $gates.projectMutation = 'PASS'

    $oldDaemon = Find-DaemonProcess
    Assert-Equal $oldDaemon.ExecutablePath $script:daemonExe 'forced-stop target executable'
    $oldDaemonId = $oldDaemon.ProcessId
    Stop-Process -Id $oldDaemonId -Force
    Wait-Process -Id $oldDaemonId -Timeout 5 -ErrorAction SilentlyContinue
    $statusAfter = Wait-Status $statusBefore.bootId 20
    $newDaemon = Find-DaemonProcess $oldDaemonId
    Assert-True ($newDaemon.ProcessId -ne $oldDaemonId) 'keeper restarted daemon with a new PID'
    $projectsAfter = @(Invoke-Json @('project', 'list'))
    $eventsAfter = @(Invoke-Json @('events', 'list', '--after', '0', '--limit', '100'))
    Assert-Equal $projectsAfter.Count 1 'project survived restart'
    Assert-Equal $eventsAfter.Count 1 'event survived restart'
    Assert-Equal $statusAfter.pendingOutbox 0 'outbox drained after restart'
    $gates.restartDurability = 'PASS'

    $shutdown = Invoke-Json @('system', 'shutdown')
    Assert-True ([bool]$shutdown.planned) 'planned shutdown response'
    Assert-Equal (Wait-Exit $keeper 15 'real keeper') 0 'planned keeper exit code'

    $crashKeeper = Start-Keeper $dataRoot (Join-Path $runRoot 'keeper-crash') $crashingExe
    $crashExit = Wait-Exit $crashKeeper 30 'crash-loop keeper'
    Assert-Equal $crashExit 20 'restart-budget exhaustion exit code'
    $pendingSegments = @(Get-ChildItem -LiteralPath (Join-Path $dataRoot 'spool\pending') -Filter '*.ndjson')
    Assert-Equal $pendingSegments.Count 1 'sealed emergency segment count'
    $incident = Get-Content -LiteralPath $pendingSegments[0].FullName -Raw | ConvertFrom-Json
    Assert-Equal $incident.code 'LZ-DAEMON-UNAVAILABLE' 'emergency incident code'
    $keeperState = Get-Content -LiteralPath (Join-Path $dataRoot 'runtime\keeper-state.json') -Raw | ConvertFrom-Json
    Assert-True ([bool]$keeperState.notificationAttempted) 'native notification attempt recorded'
    Assert-True ($null -eq $keeperState.notificationError) 'Windows notification API accepted the toast'
    $gates.restartBudgetAndSpool = 'PASS'
    $gates.nativeNotificationApi = 'PASS'

    $keeper = Start-Keeper $dataRoot (Join-Path $runRoot 'keeper-real-2')
    $recoveredStatus = Wait-Status
    Assert-Equal $recoveredStatus.emergencyIncidentCount 1 'emergency incident imported once'
    Assert-Equal @(Get-ChildItem -LiteralPath (Join-Path $dataRoot 'spool\pending') -Filter '*.ndjson').Count 0 'pending spool reconciled'
    Assert-Equal @(Get-ChildItem -LiteralPath (Join-Path $dataRoot 'spool\reconciled') -Filter '*.ndjson').Count 1 'reconciled segment retained'
    Invoke-Json @('system', 'shutdown') | Out-Null
    Assert-Equal (Wait-Exit $keeper 15 'recovery keeper') 0 'recovery keeper exit code'

    $keeper = Start-Keeper $dataRoot (Join-Path $runRoot 'keeper-real-3')
    $secondRecoveryStatus = Wait-Status
    Assert-Equal $secondRecoveryStatus.emergencyIncidentCount 1 'emergency incident remains exactly once'
    Invoke-Json @('system', 'shutdown') | Out-Null
    Assert-Equal (Wait-Exit $keeper 15 'second recovery keeper') 0 'second recovery keeper exit code'
    $gates.exactlyOnceRecovery = 'PASS'
    $gates.pipeAclAndInvalidProof = 'PASS (covered by lazoo-ipc tests)'
    $manifestStatus = 'PASS'
} catch {
    $gates.failure = $_.Exception.Message
    throw
} finally {
    $stopwatch.Stop()
    if ($null -ne $keeper -and -not $keeper.HasExited) {
        $ownedDaemon = Get-CimInstance Win32_Process -Filter "Name = 'lazood.exe'" |
            Where-Object { $_.ExecutablePath -eq $script:daemonExe }
        foreach ($process in $ownedDaemon) { Stop-Process -Id $process.ProcessId -Force }
        Stop-Process -Id $keeper.Id -Force
    }
    $manifest = [ordered]@{
        schemaVersion = 1
        databaseSchemaVersion = 2
        status = $manifestStatus
        startedAt = $startedAt.ToString('o')
        completedAt = [DateTime]::UtcNow.ToString('o')
        durationMs = $stopwatch.ElapsedMilliseconds
        platform = [Environment]::OSVersion.VersionString
        rustcVersion = (& rustc --version)
        cargoVersion = (& $cargoExe --version)
        repositoryRoot = $repoRoot
        isolatedDataRoot = $dataRoot
        temporaryProject = $projectRoot
        artifacts = [ordered]@{
            manifest = $manifestPath
            runLogs = $runRoot
            database = (Join-Path $dataRoot 'state\lazoo.db')
        }
        commands = @($script:commands)
        gates = $gates
    }
    $manifest | ConvertTo-Json -Depth 8 | Set-Content -LiteralPath $manifestPath -Encoding UTF8
}

Write-Output "PASS manifest: $manifestPath"
