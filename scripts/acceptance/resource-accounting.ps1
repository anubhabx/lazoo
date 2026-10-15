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
    $OutputPath = Join-Path $artifactDir 'resource-accounting-manifest.json'
}
$manifestPath = [IO.Path]::GetFullPath($OutputPath)
$manifestDirectory = Split-Path -Parent $manifestPath
New-Item -ItemType Directory -Force -Path $manifestDirectory | Out-Null

$startedAt = [DateTime]::UtcNow
$stopwatch = [Diagnostics.Stopwatch]::StartNew()
$commands = [Collections.Generic.List[object]]::new()
$manifestStatus = 'FAIL'
$evidence = [ordered]@{
    jobAccountingReportsTheTree = $false
    jobAccountingSurvivesTermination = $false
    processAccountingReportsItself = $false
    daemonServesResources = $false
    sessionResourcesOnlyWhileLive = $false
    providerUseSeparateFromDaemonUse = $false
    resourcesNeverStored = $false
    snapshotUnchanged = $false
    cliMapsResourceCommands = $false
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
    Invoke-Test 'lazoo-runtime' 'process_job' 'job_accounting_reports_the_tree_and_survives_its_termination'
    $evidence.jobAccountingReportsTheTree = $true
    $evidence.jobAccountingSurvivesTermination = $true

    Invoke-Test 'lazoo-runtime' 'process_job' 'current_process_accounting_reports_this_process'
    $evidence.processAccountingReportsItself = $true

    Invoke-Test 'lazooctl' 'cli' 'session_and_system_cli_map_resources'
    $evidence.cliMapsResourceCommands = $true

    Invoke-Test 'lazood' 'daemon' 'the_user_reads_resource_use_for_the_daemon_and_a_live_session'
    $evidence.daemonServesResources = $true
    $evidence.sessionResourcesOnlyWhileLive = $true
    $evidence.providerUseSeparateFromDaemonUse = $true
    $evidence.resourcesNeverStored = $true
    $evidence.snapshotUnchanged = $true

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
        throw 'desktop UI changes are outside the resource accounting slice'
    }
    $evidence.uiChangesAbsent = $true
    $manifestStatus = 'PASS'
} finally {
    Pop-Location
    $stopwatch.Stop()
    $manifest = [ordered]@{
        status = $manifestStatus
        schemaVersion = 18
        mode = 'deterministic-fake-transport'
        startedAt = $startedAt.ToString('o')
        durationMs = [long]$stopwatch.ElapsedMilliseconds
        commands = @($commands)
        evidence = $evidence
        boundaries = [ordered]@{
            providerSmoke = 'NOT_RUN'
            externalModelUse = 'NOT_RUN'
            desktopUi = 'UNCHANGED'
            schemaMigration = 'NONE'
            storage = 'POINT_SAMPLES_NEVER_STORED'
            enforcement = 'NONE'
            ceiling = 'NOT_MEASURED_OVER_ROLLING_WINDOWS'
            providerUse = 'DISPLAYED_NEVER_COUNTED_AGAINST_LAZOO'
            toolAccess = 'NONE'
        }
    }
    $manifest | ConvertTo-Json -Depth 8 | Set-Content -LiteralPath $manifestPath -Encoding utf8
}

Write-Host "PASS manifest: $manifestPath"
