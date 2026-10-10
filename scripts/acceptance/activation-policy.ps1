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
    $OutputPath = Join-Path $artifactDir 'activation-policy-manifest.json'
}
$manifestPath = [IO.Path]::GetFullPath($OutputPath)
$manifestDirectory = Split-Path -Parent $manifestPath
New-Item -ItemType Directory -Force -Path $manifestDirectory | Out-Null

$startedAt = [DateTime]::UtcNow
$stopwatch = [Diagnostics.Stopwatch]::StartNew()
$commands = [Collections.Generic.List[object]]::new()
$manifestStatus = 'FAIL'
$evidence = [ordered]@{
    policyContractIsKebabCase = $false
    policyDefaultsToManual = $false
    domainRuleActivatesOnlyNoticeTurns = $false
    policyChangeBumpsConfigurationRevision = $false
    legacyRowsStayManual = $false
    cliMapsActivationPolicy = $false
    noticeTurnActivatesByPolicy = $false
    sessionStoppedAfterTurn = $false
    ownTaskOnly = $false
    failedActivationNeverRetried = $false
    oneActivationPerRecordedTurn = $false
    userResumeStillDelivers = $false
    manualPolicyNeverActivates = $false
    peerMessagesNeverActivateManualRecipient = $false
    schemaTen = $false
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
    Invoke-Test 'lazoo-protocol' 'contracts' 'activation_policies_round_trip_as_kebab_case_and_default_to_manual'
    $evidence.policyContractIsKebabCase = $true
    $evidence.policyDefaultsToManual = $true

    Invoke-Test 'lazoo-domain' 'delivery' 'notice_turns_activate_a_recipient_only_under_the_notice_turns_policy'
    $evidence.domainRuleActivatesOnlyNoticeTurns = $true

    Invoke-Test 'lazoo-store' 'employee' 'activation_policy_defaults_to_manual_and_a_change_bumps_the_configuration_revision'
    $evidence.policyChangeBumpsConfigurationRevision = $true
    $evidence.legacyRowsStayManual = $true

    Invoke-Test 'lazooctl' 'cli' 'employee_cli_maps_mutations_and_roster_refresh'
    $evidence.cliMapsActivationPolicy = $true

    Invoke-Test 'lazood' 'daemon' 'a_notice_turn_activates_a_dormant_lead_by_policy_and_stops_the_session_after_it'
    $evidence.noticeTurnActivatesByPolicy = $true
    $evidence.sessionStoppedAfterTurn = $true
    $evidence.ownTaskOnly = $true

    Invoke-Test 'lazood' 'daemon' 'a_failed_activation_is_never_retried_and_the_user_still_resumes_the_lead'
    $evidence.failedActivationNeverRetried = $true
    $evidence.oneActivationPerRecordedTurn = $true
    $evidence.userResumeStillDelivers = $true

    Invoke-Test 'lazood' 'daemon' 'notices_wait_for_a_dormant_lead_until_the_user_resumes_it'
    $evidence.manualPolicyNeverActivates = $true

    Invoke-Test 'lazood' 'daemon' 'peer_messages_never_activate_a_dormant_recipient_and_refusals_never_end_the_turn'
    $evidence.peerMessagesNeverActivateManualRecipient = $true

    Invoke-Test 'lazoo-store' 'store' 'store_enables_wal_foreign_keys_and_schema_version'
    $evidence.schemaTen = $true

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
        throw 'desktop UI changes are outside the activation policy slice'
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
            schemaMigration = '0010_activation_policy'
            defaultActivationPolicy = 'MANUAL'
            automaticRecipientActivation = 'POLICY_BOUNDED'
            activationsPerRecordedTurn = 'ONE'
            activatedSessionAfterTurn = 'STOPPED'
            failedActivationRetry = 'NEVER'
            toolCatalog = 'UNCHANGED'
        }
    }
    $manifest | ConvertTo-Json -Depth 8 | Set-Content -LiteralPath $manifestPath -Encoding utf8
}

Write-Host "PASS manifest: $manifestPath"
