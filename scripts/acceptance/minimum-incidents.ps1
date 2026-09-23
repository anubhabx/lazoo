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
    $OutputPath = Join-Path $artifactDir 'minimum-incidents-manifest.json'
}
$manifestPath = [IO.Path]::GetFullPath($OutputPath)
$manifestDirectory = Split-Path -Parent $manifestPath
New-Item -ItemType Directory -Force -Path $manifestDirectory | Out-Null

$startedAt = [DateTime]::UtcNow
$stopwatch = [Diagnostics.Stopwatch]::StartNew()
$commands = [Collections.Generic.List[object]]::new()
$manifestStatus = 'FAIL'
$evidence = [ordered]@{
    humanReadableTaxonomy = $false
    thresholdWindow = $false
    ledgerFoldsEscalatesAndReplays = $false
    acknowledgeResolveRevisioned = $false
    emergencySpoolFoldsOnce = $false
    sessionStartFailureRaises = $false
    providerFailuresFoldAndEscalate = $false
    deadLetterAndDispatchFailureRaise = $false
    restartOwnerLossRaisesOnce = $false
    employeeToolsScoped = $false
    cliMappingAndExitCode = $false
    schemaNine = $false
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
    Invoke-Test 'lazoo-domain' 'incident' 'known_codes_describe_the_situation_and_the_next_safe_action'
    $evidence.humanReadableTaxonomy = $true
    Invoke-Test 'lazoo-domain' 'incident' 'threshold_window_promotes_three_equivalent_failures_inside_five_minutes'
    $evidence.thresholdWindow = $true

    Invoke-Test 'lazoo-store' 'incident' 'equivalent_failures_fold_escalate_at_the_threshold_and_replay_without_writes'
    $evidence.ledgerFoldsEscalatesAndReplays = $true
    Invoke-Test 'lazoo-store' 'incident' 'acknowledge_and_resolve_are_revisioned_idempotent_and_recurrence_reopens_until_resolved'
    $evidence.acknowledgeResolveRevisioned = $true
    Invoke-Test 'lazoo-store' 'incident' 'emergency_spool_records_fold_into_keeper_incidents_exactly_once'
    $evidence.emergencySpoolFoldsOnce = $true

    Invoke-Test 'lazood' 'daemon' 'session_start_failure_raises_a_human_readable_incident'
    $evidence.sessionStartFailureRaises = $true
    Invoke-Test 'lazood' 'daemon' 'provider_turn_failures_fold_into_one_incident_escalate_and_settle_through_rpc'
    $evidence.providerFailuresFoldAndEscalate = $true
    Invoke-Test 'lazood' 'daemon' 'dead_letters_and_dispatch_failures_raise_incidents_with_their_scope'
    $evidence.deadLetterAndDispatchFailureRaise = $true
    Invoke-Test 'lazood' 'daemon' 'restart_owner_loss_and_emergency_spool_fold_into_startup_incidents_once'
    $evidence.restartOwnerLossRaisesOnce = $true
    Invoke-Test 'lazood' 'daemon' 'incident_tools_report_and_list_within_the_caller_scope'
    $evidence.employeeToolsScoped = $true

    Invoke-Test 'lazooctl' 'cli' 'incident_cli_maps_list_show_acknowledge_and_resolve'
    Invoke-Test 'lazooctl' 'cli' 'incident_errors_use_exit_code_fifteen'
    $evidence.cliMappingAndExitCode = $true

    Invoke-Test 'lazoo-store' 'store' 'store_enables_wal_foreign_keys_and_schema_version'
    $evidence.schemaNine = $true

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
        throw 'desktop UI changes are outside the minimum incidents slice'
    }
    $evidence.uiChangesAbsent = $true
    $manifestStatus = 'PASS'
} finally {
    Pop-Location
    $stopwatch.Stop()
    $manifest = [ordered]@{
        status = $manifestStatus
        schemaVersion = 9
        mode = 'deterministic-fake-transport'
        startedAt = $startedAt.ToString('o')
        durationMs = [long]$stopwatch.ElapsedMilliseconds
        commands = @($commands)
        evidence = $evidence
        boundaries = [ordered]@{
            providerSmoke = 'NOT_RUN'
            externalModelUse = 'NOT_RUN'
            desktopUi = 'UNCHANGED'
            leadNotification = 'DEFERRED'
            boundedRemediation = 'DEFERRED'
            userNotification = 'DEFERRED'
        }
    }
    $manifest | ConvertTo-Json -Depth 8 | Set-Content -LiteralPath $manifestPath -Encoding utf8
}

Write-Host "PASS manifest: $manifestPath"
