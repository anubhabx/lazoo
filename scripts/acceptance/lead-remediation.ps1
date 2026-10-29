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
    $OutputPath = Join-Path $artifactDir 'lead-remediation-manifest.json'
}
$manifestPath = [IO.Path]::GetFullPath($OutputPath)
$manifestDirectory = Split-Path -Parent $manifestPath
New-Item -ItemType Directory -Force -Path $manifestDirectory | Out-Null

$startedAt = [DateTime]::UtcNow
$stopwatch = [Diagnostics.Stopwatch]::StartNew()
$commands = [Collections.Generic.List[object]]::new()
$manifestStatus = 'FAIL'
$evidence = [ordered]@{
    verdictTableCovered = $false
    noticeNamesRemediationWindow = $false
    promotionOpensLeadWindow = $false
    twoRemediesPerIncident = $false
    refusedRemedyConsumesNoAttempt = $false
    remedyActsOnlyOnDirectChildOfLeadRoot = $false
    cancelRecordsLeadAsActor = $false
    redelegationUsesDelegationGuards = $false
    recurrenceAfterSecondRemedyEscalatesToUser = $false
    unremediedWindowEscalatesToUser = $false
    heldRemedyBecomesRemediated = $false
    remediatedRecurrenceEscalatesToUser = $false
    noLeadEscalatesAtPromotion = $false
    upgradedPriorityIncidentsGoToUser = $false
    cliFiltersByEscalation = $false
    remedyToolAnswersInsideNoticeTurn = $false
    escalationChangesNoWork = $false
    toolCatalogDeclaresRemedy = $false
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
    Invoke-Test 'lazoo-domain' 'incident' 'remediation_escalates_unless_a_remedy_holds_through_the_window'
    $evidence.verdictTableCovered = $true

    Invoke-Test 'lazoo-domain' 'delivery' 'incident_notice_states_the_code_title_action_and_boundary'
    $evidence.noticeNamesRemediationWindow = $true

    Invoke-Test 'lazoo-store' 'delivery' 'a_promoted_child_incident_gives_the_lead_two_remedies_inside_its_window'
    $evidence.promotionOpensLeadWindow = $true
    $evidence.twoRemediesPerIncident = $true
    $evidence.refusedRemedyConsumesNoAttempt = $true
    $evidence.remedyActsOnlyOnDirectChildOfLeadRoot = $true
    $evidence.cancelRecordsLeadAsActor = $true
    $evidence.redelegationUsesDelegationGuards = $true
    $evidence.recurrenceAfterSecondRemedyEscalatesToUser = $true

    Invoke-Test 'lazoo-store' 'delivery' 'the_sweep_escalates_an_unremedied_window_and_keeps_a_remedy_that_held'
    $evidence.unremediedWindowEscalatesToUser = $true
    $evidence.heldRemedyBecomesRemediated = $true
    $evidence.remediatedRecurrenceEscalatesToUser = $true

    Invoke-Test 'lazoo-store' 'delivery' 'incidents_with_no_lead_to_remediate_go_to_the_user_at_promotion'
    $evidence.noLeadEscalatesAtPromotion = $true

    Invoke-Test 'lazoo-store' 'store' 'version_eleven_priority_incidents_upgrade_straight_to_the_user'
    $evidence.upgradedPriorityIncidentsGoToUser = $true

    Invoke-Test 'lazooctl' 'cli' 'incident_cli_maps_list_show_acknowledge_and_resolve'
    $evidence.cliFiltersByEscalation = $true

    Invoke-Test 'lazood' 'daemon' 'a_lead_remedies_a_promoted_child_incident_in_its_notice_turn_within_the_bound'
    $evidence.remedyToolAnswersInsideNoticeTurn = $true

    Invoke-Test 'lazood' 'daemon' 'an_unremedied_window_reaches_the_user_on_the_sweep_and_nothing_else_changes'
    $evidence.escalationChangesNoWork = $true

    Invoke-Test 'lazood' 'daemon' 'lead_turn_lists_employees_delegates_and_waits_through_collaboration_tools'
    $evidence.toolCatalogDeclaresRemedy = $true

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
        throw 'desktop UI changes are outside the lead remediation slice'
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
            schemaMigration = '0012_lead_remediation'
            remediesPerIncident = 'TWO'
            remediationWindowSeconds = 300
            remedies = 'CANCEL_TASK,REDELEGATE_TASK'
            remedyScope = 'DIRECT_CHILD_OF_THE_LEAD_ROOT_TASK'
            unresolvedAfterWindow = 'ESCALATED_TO_USER'
            incidentResolution = 'USER_ONLY'
            automaticRetry = 'NEVER'
            toolCatalog = 'NINE_TOOLS_WITH_INCIDENT_REMEDY'
        }
    }
    $manifest | ConvertTo-Json -Depth 8 | Set-Content -LiteralPath $manifestPath -Encoding utf8
}

Write-Host "PASS manifest: $manifestPath"
