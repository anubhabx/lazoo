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
        $captured = @(& $script:ctlExe --json @Arguments 2>&1)
        $exitCode = $LASTEXITCODE
        $script:commands.Add([ordered]@{
            command = "lazooctl --json $($Arguments -join ' ')"
            exitCode = $exitCode
            durationMs = $null
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
$manifestPath = Join-Path $artifactDir 'employee-roster-manifest.json'
$runId = [Guid]::NewGuid().ToString('N')
$tempBase = [IO.Path]::GetFullPath([IO.Path]::GetTempPath())
$runRoot = [IO.Path]::GetFullPath((Join-Path $tempBase "lazoo-employee-roster-$runId"))
if (-not $runRoot.StartsWith($tempBase, [StringComparison]::OrdinalIgnoreCase)) {
    throw 'acceptance run root is outside the system temporary directory'
}
$script:dataRoot = Join-Path $runRoot 'data'
$startedAt = [DateTime]::UtcNow
$stopwatch = [Diagnostics.Stopwatch]::StartNew()
$script:commands = [Collections.Generic.List[object]]::new()
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
    foreach ($binary in @($script:daemonExe, $script:ctlExe)) {
        Assert-True (Test-Path -LiteralPath $binary) "binary exists: $binary"
    }
    $gates.releaseBuild = if ($SkipBuild) { 'SKIPPED_BY_CALLER' } else { 'PASS' }

    $daemon = Start-Daemon
    $status = Wait-Status
    Assert-Equal $status.protocolVersion 1 'protocol version'
    Assert-Equal $status.schemaVersion 5 'schemaVersion 5'
    Assert-Equal $status.journalMode 'wal' 'SQLite journal mode'
    $gates.schemaVersion = 'PASS'

    $createKey = "employee-create-$runId"
    $first = Invoke-Json @(
        'employee', 'create', 'Rhea',
        '--provider', 'codex',
        '--role', 'Lead',
        '--idempotency-key', $createKey
    )
    $replay = Invoke-Json @(
        'employee', 'create', 'Rhea',
        '--provider', 'codex',
        '--role', 'Lead',
        '--idempotency-key', $createKey
    )
    Assert-Equal $first.employee.id $replay.employee.id 'idempotent employee identity'
    Assert-Equal $first.employee.configurationRevision 1 'initial employee revision'
    Assert-Equal $first.rosterRevision 1 'initial roster revision'
    $employeeId = [string]$first.employee.id

    $listed = @(Invoke-Json @('employee', 'list'))
    Assert-Equal $listed.Count 1 'employee count'
    $roster = Invoke-Json @('roster', 'show')
    Assert-Equal $roster.revision 1 'initial roster read revision'
    Assert-True (@('disabled', 'offline', 'unverified', 'dormant') -contains [string]$roster.entries[0].availability) 'truthful availability state'
    $gates.atomicCreateAndReplay = 'PASS'

    $firstBootId = [string]$status.bootId
    Stop-Daemon $daemon
    $daemon = Start-Daemon
    $afterRestart = Wait-Status $firstBootId
    $shownAfterRestart = Invoke-Json @('employee', 'show', $employeeId)
    Assert-Equal $shownAfterRestart.name 'Rhea' 'employee survived restart'
    Assert-Equal $shownAfterRestart.configurationRevision 1 'employee revision survived restart'
    Assert-True ($afterRestart.bootId -ne $firstBootId) 'daemon boot identity changed'
    $gates.restartDurability = 'PASS'

    $updated = Invoke-Json @(
        'employee', 'update', $employeeId,
        '--expected-revision', '1',
        '--role', 'Principal lead',
        '--idempotency-key', "employee-update-$runId"
    )
    Assert-Equal $updated.employee.configurationRevision 2 'updated employee revision'
    Assert-Equal $updated.employee.role 'Principal lead' 'updated employee role'
    Assert-True ($updated.rosterRevision -gt 1) 'roster revision advanced after update'

    $stale = Invoke-ExpectedFailure 9 @(
        'employee', 'update', $employeeId,
        '--expected-revision', '1',
        '--role', 'Stale edit',
        '--idempotency-key', "employee-stale-$runId"
    )
    Assert-True $stale.Contains('LZ-EMPLOYEE-STALE') 'stale error code'
    Assert-True $stale.Contains('currentRevision') 'stale current revision evidence'
    $gates.staleRevisionRejected = 'PASS'

    $refreshed = Invoke-Json @(
        'roster', 'refresh',
        '--idempotency-key', "roster-refresh-$runId"
    )
    Assert-True ($refreshed.roster.revision -ge $updated.rosterRevision) 'roster revision is monotonic'
    $gates.nonModelRefresh = 'PASS'
    $gates.providerSmokeAbsent = 'PASS'

    $disabled = Invoke-Json @(
        'employee', 'disable', $employeeId,
        '--expected-revision', '2',
        '--idempotency-key', "employee-disable-$runId"
    )
    Assert-Equal $disabled.employee.enabled $false 'employee disabled'
    Assert-Equal $disabled.employee.configurationRevision 3 'disabled employee revision'
    $disabledRoster = Invoke-Json @('roster', 'show')
    Assert-Equal $disabledRoster.entries[0].availability 'disabled' 'disabled roster state'

    $secondBootId = [string](Invoke-Json @('status')).bootId
    Stop-Daemon $daemon
    $daemon = Start-Daemon
    Wait-Status $secondBootId | Out-Null
    $preserved = Invoke-Json @('employee', 'show', $employeeId)
    Assert-Equal $preserved.enabled $false 'disabled identity survived restart'
    Assert-Equal $preserved.role 'Principal lead' 'employee history survived restart'
    $gates.disabledIdentityPreserved = 'PASS'

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
        schemaVersion = 5
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
