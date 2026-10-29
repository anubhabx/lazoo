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
    $OutputPath = Join-Path $artifactDir 'configurable-thresholds-manifest.json'
}
$manifestPath = [IO.Path]::GetFullPath($OutputPath)
$manifestDirectory = Split-Path -Parent $manifestPath
New-Item -ItemType Directory -Force -Path $manifestDirectory | Out-Null

$startedAt = [DateTime]::UtcNow
$stopwatch = [Diagnostics.Stopwatch]::StartNew()
$commands = [Collections.Generic.List[object]]::new()
$manifestStatus = 'FAIL'
$evidence = [ordered]@{
    thresholdContractsStrict = $false
    overrideReplacesDefault = $false
    lossesPromoteWhateverIsConfigured = $false
    configuredWindowApplies = $false
    overrideAppliesFromNextOccurrence = $false
    resetRestoresDefault = $false
    promotedIncidentsNeverDemoted = $false
    unchangedValueEmitsNoEvent = $false
    rangesValidated = $false
    cliMapsThresholdCommands = $false
    daemonServesThresholds = $false
    toolsCannotSeeThresholds = $false
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
    Invoke-Test 'lazoo-protocol' 'incident' 'incident_threshold_contracts_are_strict_and_default_to_five_minutes'
    $evidence.thresholdContractsStrict = $true

    Invoke-Test 'lazoo-domain' 'incident' 'a_configured_threshold_replaces_the_default_but_never_delays_a_loss'
    $evidence.overrideReplacesDefault = $true
    $evidence.lossesPromoteWhateverIsConfigured = $true

    Invoke-Test 'lazoo-store' 'incident' 'configured_thresholds_apply_from_the_next_occurrence_and_reset_to_the_default'
    $evidence.configuredWindowApplies = $true
    $evidence.overrideAppliesFromNextOccurrence = $true
    $evidence.resetRestoresDefault = $true
    $evidence.promotedIncidentsNeverDemoted = $true
    $evidence.unchangedValueEmitsNoEvent = $true
    $evidence.rangesValidated = $true

    Invoke-Test 'lazooctl' 'cli' 'incident_cli_maps_threshold_list_set_and_reset'
    $evidence.cliMapsThresholdCommands = $true

    Invoke-Test 'lazood' 'daemon' 'the_user_configures_incident_thresholds_through_the_daemon'
    $evidence.daemonServesThresholds = $true
    $evidence.toolsCannotSeeThresholds = $true

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
        throw 'desktop UI changes are outside the configurable thresholds slice'
    }
    $evidence.uiChangesAbsent = $true
    $manifestStatus = 'PASS'
} finally {
    Pop-Location
    $stopwatch.Stop()
    $manifest = [ordered]@{
        status = $manifestStatus
        schemaVersion = 20
        mode = 'deterministic-fake-transport'
        startedAt = $startedAt.ToString('o')
        durationMs = [long]$stopwatch.ElapsedMilliseconds
        commands = @($commands)
        evidence = $evidence
        boundaries = [ordered]@{
            providerSmoke = 'NOT_RUN'
            externalModelUse = 'NOT_RUN'
            desktopUi = 'UNCHANGED'
            schemaMigration = '0015_incident_thresholds'
            thresholdWriter = 'LOCAL_USER_THROUGH_DAEMON_AND_CLI'
            toolAccess = 'NONE'
            occurrencesRange = '1..100'
            windowSecondsRange = '60..86400'
            lossRules = 'ALWAYS_ONE'
        }
    }
    $manifest | ConvertTo-Json -Depth 8 | Set-Content -LiteralPath $manifestPath -Encoding utf8
}

Write-Host "PASS manifest: $manifestPath"
