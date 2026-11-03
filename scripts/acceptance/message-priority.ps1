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
    $OutputPath = Join-Path $artifactDir 'message-priority-manifest.json'
}
$manifestPath = [IO.Path]::GetFullPath($OutputPath)
$manifestDirectory = Split-Path -Parent $manifestPath
New-Item -ItemType Directory -Force -Path $manifestDirectory | Out-Null

$startedAt = [DateTime]::UtcNow
$stopwatch = [Diagnostics.Stopwatch]::StartNew()
$commands = [Collections.Generic.List[object]]::new()
$manifestStatus = 'FAIL'
$evidence = [ordered]@{
    priorityFieldRoundTrips = $false
    olderRecordsDefaultToNormal = $false
    priorityDeadLetterPromotedAtOnce = $false
    normalDeadLetterKeepsDefaultThreshold = $false
    priorityReadFromTheDeadLetter = $false
    oneIncidentPerDeadLetter = $false
    incidentNoticesArePriority = $false
    lostNoticeReachesTheUserInOneSweep = $false
    userCanSendPriority = $false
    toolsCannotSetPriority = $false
    upgradeMarksIncidentNoticesPriority = $false
    schemaFourteen = $false
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
    Invoke-Test 'lazoo-protocol' 'contracts' 'message_priority_is_kebab_case_and_defaults_to_normal_for_older_records'
    $evidence.priorityFieldRoundTrips = $true
    $evidence.olderRecordsDefaultToNormal = $true

    Invoke-Test 'lazoo-domain' 'incident' 'section_fourteen_two_thresholds_promote_stalls_crashes_and_deaths_at_once'
    $evidence.normalDeadLetterKeepsDefaultThreshold = $true

    Invoke-Test 'lazoo-store' 'incident' 'one_dead_lettered_priority_message_is_a_priority_incident_raised_once'
    $evidence.priorityDeadLetterPromotedAtOnce = $true
    $evidence.priorityReadFromTheDeadLetter = $true
    $evidence.oneIncidentPerDeadLetter = $true

    Invoke-Test 'lazoo-store' 'store' 'version_thirteen_incident_notices_upgrade_as_priority_messages'
    $evidence.upgradeMarksIncidentNoticesPriority = $true

    Invoke-Test 'lazooctl' 'cli' 'message_cli_maps_send_envelope'
    Invoke-Test 'lazood' 'daemon' 'message_fail_promotes_a_priority_dead_letter_at_once_and_only_once'
    $evidence.userCanSendPriority = $true

    Invoke-Test 'lazood' 'daemon' 'a_dead_lettered_incident_notice_reaches_the_user_in_one_sweep'
    $evidence.incidentNoticesArePriority = $true
    $evidence.lostNoticeReachesTheUserInOneSweep = $true

    $toolsSource = Get-Content -Raw -LiteralPath (Join-Path $repoRoot 'apps/lazood/src/app/tools.rs')
    $commands.Add([ordered]@{
        command = 'Select-String MessagePriority apps/lazood/src/app/tools.rs'
        exitCode = 0
        durationMs = $null
    })
    if ($toolsSource.Contains('MessagePriority')) {
        throw 'a collaboration tool can set message priority'
    }
    $evidence.toolsCannotSetPriority = $true

    Invoke-Test 'lazoo-store' 'store' 'store_enables_wal_foreign_keys_and_schema_version'
    $evidence.schemaFourteen = $true

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
        throw 'desktop UI changes are outside the message priority slice'
    }
    $evidence.uiChangesAbsent = $true
    $manifestStatus = 'PASS'
} finally {
    Pop-Location
    $stopwatch.Stop()
    $manifest = [ordered]@{
        status = $manifestStatus
        schemaVersion = 22
        mode = 'deterministic-fake-transport'
        startedAt = $startedAt.ToString('o')
        durationMs = [long]$stopwatch.ElapsedMilliseconds
        commands = @($commands)
        evidence = $evidence
        boundaries = [ordered]@{
            providerSmoke = 'NOT_RUN'
            externalModelUse = 'NOT_RUN'
            desktopUi = 'UNCHANGED'
            schemaMigration = '0014_message_priority'
            priorityDeadLetterThreshold = 'ONE'
            prioritySources = 'INCIDENT_NOTICES_AND_USER_MESSAGE_SEND'
            toolSetPriority = 'NONE'
            deliveryOrder = 'UNCHANGED'
        }
    }
    $manifest | ConvertTo-Json -Depth 8 | Set-Content -LiteralPath $manifestPath -Encoding utf8
}

Write-Host "PASS manifest: $manifestPath"
