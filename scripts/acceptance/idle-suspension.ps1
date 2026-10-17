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
    $OutputPath = Join-Path $artifactDir 'idle-suspension-manifest.json'
}
$manifestPath = [IO.Path]::GetFullPath($OutputPath)
$manifestDirectory = Split-Path -Parent $manifestPath
New-Item -ItemType Directory -Force -Path $manifestDirectory | Out-Null

$startedAt = [DateTime]::UtcNow
$stopwatch = [Diagnostics.Stopwatch]::StartNew()
$commands = [Collections.Generic.List[object]]::new()
$manifestStatus = 'FAIL'
$evidence = [ordered]@{
    timeoutDefaultsToNever = $false
    updateWithoutTimeoutKeepsIt = $false
    timeoutBoundedToAMinuteThroughADay = $false
    timeoutChangeBumpsConfigurationRevision = $false
    legacyRowsNeverSuspend = $false
    cliMapsIdleTimeout = $false
    dueOnlyAfterWholeTimeout = $false
    otherBootSessionsUntouched = $false
    oneStopPerSessionKey = $false
    quietSessionSuspended = $false
    quietTimeCountsFromLastTurn = $false
    unfinishedTurnKeepsSession = $false
    zeroTimeoutNeverSuspends = $false
    nextUserTurnStartsFreshSession = $false
    recordedTurnNotStranded = $false
    suspendedNoticeTurnsLeadWokenOnce = $false
    schemaEleven = $false
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
    Invoke-Test 'lazoo-protocol' 'contracts' 'idle_timeouts_default_to_never_on_create_and_stay_unchanged_when_an_update_omits_them'
    $evidence.timeoutDefaultsToNever = $true
    $evidence.updateWithoutTimeoutKeepsIt = $true

    Invoke-Test 'lazoo-domain' 'employee' 'idle_timeouts_are_never_or_a_minute_to_a_day_and_due_only_after_the_whole_timeout'
    $evidence.timeoutBoundedToAMinuteThroughADay = $true

    Invoke-Test 'lazoo-store' 'employee' 'idle_timeout_defaults_to_never_and_a_change_bumps_the_configuration_revision'
    $evidence.timeoutChangeBumpsConfigurationRevision = $true
    $evidence.legacyRowsNeverSuspend = $true

    Invoke-Test 'lazoo-store' 'session' 'idle_sessions_are_due_only_after_their_employee_timeout_and_stop_once_under_their_own_key'
    $evidence.dueOnlyAfterWholeTimeout = $true
    $evidence.otherBootSessionsUntouched = $true
    $evidence.oneStopPerSessionKey = $true

    Invoke-Test 'lazooctl' 'cli' 'employee_cli_maps_mutations_and_roster_refresh'
    $evidence.cliMapsIdleTimeout = $true

    Invoke-Test 'lazood' 'daemon' 'an_idle_session_suspends_after_its_employee_timeout_and_the_next_user_turn_starts_fresh'
    $evidence.quietSessionSuspended = $true
    $evidence.nextUserTurnStartsFreshSession = $true

    Invoke-Test 'lazood' 'daemon' 'idle_suspension_waits_for_an_unfinished_turn_and_never_touches_a_zero_timeout'
    $evidence.unfinishedTurnKeepsSession = $true
    $evidence.quietTimeCountsFromLastTurn = $true
    $evidence.zeroTimeoutNeverSuspends = $true

    Invoke-Test 'lazood' 'daemon' 'a_turn_recorded_on_a_since_stopped_session_is_delivered_on_the_current_session'
    $evidence.recordedTurnNotStranded = $true

    Invoke-Test 'lazood' 'daemon' 'a_notice_turns_lead_suspended_for_idleness_is_woken_once_by_its_childs_result'
    $evidence.suspendedNoticeTurnsLeadWokenOnce = $true

    Invoke-Test 'lazoo-store' 'store' 'store_enables_wal_foreign_keys_and_schema_version'
    $evidence.schemaEleven = $true

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
        throw 'desktop UI changes are outside the idle suspension slice'
    }
    $evidence.uiChangesAbsent = $true
    $manifestStatus = 'PASS'
} finally {
    Pop-Location
    $stopwatch.Stop()
    $manifest = [ordered]@{
        status = $manifestStatus
        schemaVersion = 19
        mode = 'deterministic-fake-transport'
        startedAt = $startedAt.ToString('o')
        durationMs = [long]$stopwatch.ElapsedMilliseconds
        commands = @($commands)
        evidence = $evidence
        boundaries = [ordered]@{
            providerSmoke = 'NOT_RUN'
            externalModelUse = 'NOT_RUN'
            desktopUi = 'UNCHANGED'
            schemaMigration = '0011_idle_suspension'
            defaultIdleTimeout = 'NEVER'
            idleTimeoutRangeSeconds = '60-86400'
            suspendedSessions = 'IDLE_OWNED_BY_THIS_BOOT_WITH_NO_UNFINISHED_TURN'
            stopsPerSession = 'ONE'
            failedStopRetry = 'NEVER'
            nativeThreadAfterSuspension = 'ENDED'
            toolCatalog = 'UNCHANGED'
        }
    }
    $manifest | ConvertTo-Json -Depth 8 | Set-Content -LiteralPath $manifestPath -Encoding utf8
}

Write-Host "PASS manifest: $manifestPath"
