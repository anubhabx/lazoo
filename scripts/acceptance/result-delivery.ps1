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
    $OutputPath = Join-Path $artifactDir 'result-delivery-manifest.json'
}
$manifestPath = [IO.Path]::GetFullPath($OutputPath)
$manifestDirectory = Split-Path -Parent $manifestPath
New-Item -ItemType Directory -Force -Path $manifestDirectory | Out-Null

$startedAt = [DateTime]::UtcNow
$stopwatch = [Diagnostics.Stopwatch]::StartNew()
$commands = [Collections.Generic.List[object]]::new()
$manifestStatus = 'FAIL'
$evidence = [ordered]@{
    noticeTextBoundedAndTruthful = $false
    childResultsQueueOnce = $false
    noticeTurnReusesMessage = $false
    taskWaitConsumesResults = $false
    foreignTurnRefused = $false
    priorityIncidentsNotifyLeadOnce = $false
    idleLeadReceivesResultTurn = $false
    taskWaitPreventsSecondDelivery = $false
    dormantLeadNeverActivated = $false
    userResumeFlushesNotices = $false
    escalatedIncidentReachesLead = $false
    schemaUnchanged = $false
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
    Invoke-Test 'lazoo-domain' 'delivery' 'completed_result_names_the_child_its_output_and_the_parent'
    Invoke-Test 'lazoo-domain' 'delivery' 'failed_result_carries_the_failure_and_the_remaining_children'
    Invoke-Test 'lazoo-domain' 'delivery' 'oversized_output_is_bounded_on_a_utf8_boundary_and_marked'
    Invoke-Test 'lazoo-domain' 'delivery' 'incident_notice_states_the_code_title_action_and_boundary'
    $evidence.noticeTextBoundedAndTruthful = $true

    Invoke-Test 'lazoo-store' 'delivery' 'terminal_child_results_queue_once_to_the_parent_lead'
    $evidence.childResultsQueueOnce = $true

    Invoke-Test 'lazoo-store' 'delivery' 'notice_turns_reuse_the_notice_message_and_wait_for_the_lead_to_be_free'
    $evidence.noticeTurnReusesMessage = $true

    Invoke-Test 'lazoo-store' 'delivery' 'task_wait_consumes_a_result_with_the_waiting_turn_as_evidence'
    $evidence.taskWaitConsumesResults = $true

    Invoke-Test 'lazoo-store' 'delivery' 'a_foreign_turn_cannot_consume_another_leads_result'
    $evidence.foreignTurnRefused = $true

    Invoke-Test 'lazoo-store' 'delivery' 'priority_incidents_on_a_child_notify_the_lead_once'
    $evidence.priorityIncidentsNotifyLeadOnce = $true

    Invoke-Test 'lazood' 'daemon' 'child_results_reach_an_idle_lead_as_a_notice_turn_after_its_turn_ended'
    $evidence.idleLeadReceivesResultTurn = $true

    Invoke-Test 'lazood' 'daemon' 'task_wait_consumes_child_results_so_no_notice_turn_follows'
    $evidence.taskWaitPreventsSecondDelivery = $true

    Invoke-Test 'lazood' 'daemon' 'notices_wait_for_a_dormant_lead_until_the_user_resumes_it'
    $evidence.dormantLeadNeverActivated = $true
    $evidence.userResumeFlushesNotices = $true

    Invoke-Test 'lazood' 'daemon' 'escalated_incidents_notify_the_lead_once_as_a_status_turn'
    $evidence.escalatedIncidentReachesLead = $true

    Invoke-Test 'lazoo-store' 'store' 'store_enables_wal_foreign_keys_and_schema_version'
    $evidence.schemaUnchanged = $true

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
        throw 'desktop UI changes are outside the result delivery slice'
    }
    $evidence.uiChangesAbsent = $true
    $manifestStatus = 'PASS'
} finally {
    Pop-Location
    $stopwatch.Stop()
    $manifest = [ordered]@{
        status = $manifestStatus
        schemaVersion = 13
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
            automaticLeadActivation = 'NEVER'
            boundedRemediation = 'DEFERRED'
            peerMessagingTools = 'DEFERRED'
        }
    }
    $manifest | ConvertTo-Json -Depth 8 | Set-Content -LiteralPath $manifestPath -Encoding utf8
}

Write-Host "PASS manifest: $manifestPath"
