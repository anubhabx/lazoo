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
    $OutputPath = Join-Path $artifactDir 'coalesced-notices-manifest.json'
}
$manifestPath = [IO.Path]::GetFullPath($OutputPath)
$manifestDirectory = Split-Path -Parent $manifestPath
New-Item -ItemType Directory -Force -Path $manifestDirectory | Out-Null

$startedAt = [DateTime]::UtcNow
$stopwatch = [Diagnostics.Stopwatch]::StartNew()
$commands = [Collections.Generic.List[object]]::new()
$manifestStatus = 'FAIL'
$evidence = [ordered]@{
    coalescedTextOldestFirst = $false
    singleNoticeTextUnchanged = $false
    oneTurnPerRecipientBatch = $false
    eachNoticeKeepsOwnEvidence = $false
    mixedTurnCompletesTask = $false
    readNoticeReleasesBatch = $false
    closedTurnNeverReused = $false
    batchBoundedByTurnTextLimit = $false
    peerOnlyTurnNeverCompletesTask = $false
    idleLeadReceivesOneTurn = $false
    singleNoticeDeliveryUnchanged = $false
    dormantLeadNeverActivated = $false
    userResumeDeliversOneTurn = $false
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
    Invoke-Test 'lazoo-domain' 'delivery' 'coalesced_notice_text_lists_every_notice_oldest_first_and_passes_one_through'
    $evidence.coalescedTextOldestFirst = $true
    $evidence.singleNoticeTextUnchanged = $true

    Invoke-Test 'lazoo-store' 'delivery' 'queued_notices_for_one_recipient_coalesce_into_one_turn_with_evidence_for_each'
    $evidence.oneTurnPerRecipientBatch = $true
    $evidence.eachNoticeKeepsOwnEvidence = $true
    $evidence.mixedTurnCompletesTask = $true

    Invoke-Test 'lazoo-store' 'delivery' 'a_notice_read_inside_another_turn_releases_its_batch_for_a_fresh_turn'
    $evidence.readNoticeReleasesBatch = $true
    $evidence.closedTurnNeverReused = $true

    Invoke-Test 'lazoo-store' 'delivery' 'a_coalesced_turn_stays_within_the_turn_text_limit'
    $evidence.batchBoundedByTurnTextLimit = $true

    Invoke-Test 'lazoo-store' 'delivery' 'peer_turns_never_complete_the_recipient_task'
    $evidence.peerOnlyTurnNeverCompletesTask = $true

    Invoke-Test 'lazood' 'daemon' 'queued_notices_reach_an_idle_lead_as_one_turn'
    $evidence.idleLeadReceivesOneTurn = $true

    Invoke-Test 'lazood' 'daemon' 'child_results_reach_an_idle_lead_as_a_notice_turn_after_its_turn_ended'
    $evidence.singleNoticeDeliveryUnchanged = $true

    Invoke-Test 'lazood' 'daemon' 'peer_messages_never_activate_a_dormant_recipient_and_refusals_never_end_the_turn'
    $evidence.dormantLeadNeverActivated = $true
    $evidence.userResumeDeliversOneTurn = $true

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
        throw 'desktop UI changes are outside the coalesced notices slice'
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
            schemaMigration = 'NONE'
            automaticRecipientActivation = 'NEVER'
            crossTreeMessaging = 'REFUSED'
            noticeTurnsPerRecipientBatch = 'ONE'
            otherProviderTools = 'NOT_DECLARED'
        }
    }
    $manifest | ConvertTo-Json -Depth 8 | Set-Content -LiteralPath $manifestPath -Encoding utf8
}

Write-Host "PASS manifest: $manifestPath"
