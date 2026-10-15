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
    $OutputPath = Join-Path $artifactDir 'incident-thresholds-manifest.json'
}
$manifestPath = [IO.Path]::GetFullPath($OutputPath)
$manifestDirectory = Split-Path -Parent $manifestPath
New-Item -ItemType Directory -Force -Path $manifestDirectory | Out-Null

$startedAt = [DateTime]::UtcNow
$stopwatch = [Diagnostics.Stopwatch]::StartNew()
$commands = [Collections.Generic.List[object]]::new()
$manifestStatus = 'FAIL'
$evidence = [ordered]@{
    thresholdTableCovered = $false
    stallPromotesOnFirstOccurrence = $false
    daemonCrashPromotesOnFirstOccurrence = $false
    secondUnexpectedExitPromotes = $false
    deathUnderAssignedWorkPromotesOnFirstOccurrence = $false
    promotedChildFailureOpensLeadWindow = $false
    otherCodesKeepThreeInFiveMinutes = $false
    keeperCrashReachesUserAtStartup = $false
    leadRemediationUnchanged = $false
    schemaTwelve = $false
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
    Invoke-Test 'lazoo-domain' 'incident' 'section_fourteen_two_thresholds_promote_stalls_crashes_and_deaths_at_once'
    $evidence.thresholdTableCovered = $true

    Invoke-Test 'lazoo-domain' 'incident' 'threshold_window_promotes_three_equivalent_failures_inside_five_minutes'
    $evidence.otherCodesKeepThreeInFiveMinutes = $true

    Invoke-Test 'lazoo-store' 'incident' 'stalls_and_crashes_promote_at_once_and_a_second_unexpected_exit_promotes'
    $evidence.stallPromotesOnFirstOccurrence = $true
    $evidence.daemonCrashPromotesOnFirstOccurrence = $true
    $evidence.secondUnexpectedExitPromotes = $true

    Invoke-Test 'lazoo-store' 'incident' 'equivalent_failures_fold_escalate_at_the_threshold_and_replay_without_writes'

    Invoke-Test 'lazoo-store' 'delivery' 'a_stall_or_a_death_under_assigned_work_opens_the_lead_window_on_its_first_occurrence'
    $evidence.deathUnderAssignedWorkPromotesOnFirstOccurrence = $true
    $evidence.promotedChildFailureOpensLeadWindow = $true

    Invoke-Test 'lazood' 'daemon' 'restart_owner_loss_and_emergency_spool_fold_into_startup_incidents_once'
    $evidence.keeperCrashReachesUserAtStartup = $true

    Invoke-Test 'lazood' 'daemon' 'an_unremedied_window_reaches_the_user_on_the_sweep_and_nothing_else_changes'
    Invoke-Test 'lazoo-store' 'delivery' 'a_promoted_child_incident_gives_the_lead_two_remedies_inside_its_window'
    $evidence.leadRemediationUnchanged = $true

    Invoke-Test 'lazoo-store' 'store' 'store_enables_wal_foreign_keys_and_schema_version'
    $evidence.schemaTwelve = $true

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
        throw 'desktop UI changes are outside the incident thresholds slice'
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
            stalledSessionThreshold = 1
            daemonCrashThreshold = 1
            processExitUnderAssignedWorkThreshold = 1
            unexpectedProcessExitThreshold = 2
            otherCodesThreshold = 3
            thresholdWindowSeconds = 300
            deadLetteredPriorityMessage = 'DEFERRED_NO_MESSAGE_PRIORITY'
            configurableThresholds = 'DEFERRED'
            automaticRetry = 'NEVER'
        }
    }
    $manifest | ConvertTo-Json -Depth 8 | Set-Content -LiteralPath $manifestPath -Encoding utf8
}

Write-Host "PASS manifest: $manifestPath"
