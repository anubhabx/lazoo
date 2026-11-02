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
    $OutputPath = Join-Path $artifactDir 'user-notifications-manifest.json'
}
$manifestPath = [IO.Path]::GetFullPath($OutputPath)
$manifestDirectory = Split-Path -Parent $manifestPath
New-Item -ItemType Directory -Force -Path $manifestDirectory | Out-Null

$startedAt = [DateTime]::UtcNow
$stopwatch = [Diagnostics.Stopwatch]::StartNew()
$commands = [Collections.Generic.List[object]]::new()
$manifestStatus = 'FAIL'
$evidence = [ordered]@{
    notificationFieldsRoundTrip = $false
    announcedAtMostOnce = $false
    onlyOpenUserEscalationsAnnounced = $false
    keeperRecordsNotAnnouncedTwice = $false
    settledOutcomeNeverRewritten = $false
    singleEscalationNamesTheIncident = $false
    burstCoalescedIntoOneNotification = $false
    leadWindowNotAnnouncedUntilEscalated = $false
    failedNotificationNeverRetried = $false
    notificationChangesNoWork = $false
    upgradeKeepsIncidentsUnannounced = $false
    schemaThirteen = $false
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
    Invoke-Test 'lazoo-protocol' 'incident' 'incident_record_round_trips_with_camel_case_keys'
    $evidence.notificationFieldsRoundTrip = $true

    Invoke-Test 'lazoo-store' 'incident' 'user_escalations_are_claimed_once_and_their_outcome_is_never_rewritten'
    $evidence.announcedAtMostOnce = $true
    $evidence.onlyOpenUserEscalationsAnnounced = $true
    $evidence.keeperRecordsNotAnnouncedTwice = $true
    $evidence.settledOutcomeNeverRewritten = $true

    Invoke-Test 'lazood' 'daemon' 'user_escalations_reach_the_desktop_once_coalesced_and_never_retried'
    $evidence.singleEscalationNamesTheIncident = $true
    $evidence.burstCoalescedIntoOneNotification = $true
    $evidence.leadWindowNotAnnouncedUntilEscalated = $true
    $evidence.failedNotificationNeverRetried = $true
    $evidence.notificationChangesNoWork = $true

    Invoke-Test 'lazoo-store' 'store' 'version_eleven_priority_incidents_upgrade_straight_to_the_user'
    $evidence.upgradeKeepsIncidentsUnannounced = $true

    Invoke-Test 'lazoo-store' 'store' 'store_enables_wal_foreign_keys_and_schema_version'
    $evidence.schemaThirteen = $true

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
        throw 'desktop UI changes are outside the user notifications slice'
    }
    $evidence.uiChangesAbsent = $true
    $manifestStatus = 'PASS'
} finally {
    Pop-Location
    $stopwatch.Stop()
    $manifest = [ordered]@{
        status = $manifestStatus
        schemaVersion = 21
        mode = 'deterministic-fake-transport'
        startedAt = $startedAt.ToString('o')
        durationMs = [long]$stopwatch.ElapsedMilliseconds
        commands = @($commands)
        evidence = $evidence
        boundaries = [ordered]@{
            providerSmoke = 'NOT_RUN'
            externalModelUse = 'NOT_RUN'
            desktopUi = 'UNCHANGED'
            schemaMigration = '0013_user_notifications'
            channel = 'WINDOWS_TOAST_FROM_THE_DAEMON'
            realNotificationsInTests = 'NONE_RECORDING_NOTIFIER'
            announcementsPerIncident = 'AT_MOST_ONE'
            notificationsPerPass = 'ONE'
            failedNotificationRetry = 'NEVER'
            keeperRecords = 'NOT_ANNOUNCED_TWICE'
        }
    }
    $manifest | ConvertTo-Json -Depth 8 | Set-Content -LiteralPath $manifestPath -Encoding utf8
}

Write-Host "PASS manifest: $manifestPath"
