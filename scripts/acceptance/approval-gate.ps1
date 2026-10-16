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
    $OutputPath = Join-Path $artifactDir 'approval-gate-manifest.json'
}
$manifestPath = [IO.Path]::GetFullPath($OutputPath)
$manifestDirectory = Split-Path -Parent $manifestPath
New-Item -ItemType Directory -Force -Path $manifestDirectory | Out-Null

$startedAt = [DateTime]::UtcNow
$stopwatch = [Diagnostics.Stopwatch]::StartNew()
$commands = [Collections.Generic.List[object]]::new()
$manifestStatus = 'FAIL'
$evidence = [ordered]@{
    policyTable = $false
    askBlocksProviderUntilUserDecision = $false
    declineExpiryCancellationFailClosed = $false
    selfDecisionScopePolicy = $false
    restartOwnerLoss = $false
    ledgerRevisionsAndIdempotency = $false
    codexAdapterAnswersNativeRequest = $false
    cliMappingAndExitCode = $false
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
    Invoke-Test 'lazoo-domain' 'approval' 'ask_always_waits_for_the_user'
    Invoke-Test 'lazoo-domain' 'approval' 'self_decision_acts_inside_scope_but_waits_on_scope_expansion'
    Invoke-Test 'lazoo-domain' 'approval' 'full_autonomy_proceeds_without_a_user_decision'
    $evidence.policyTable = $true

    Invoke-Test 'lazoo-store' 'approval' 'ask_authority_records_a_pending_approval_and_user_decisions_are_revisioned_and_idempotent'
    Invoke-Test 'lazoo-store' 'approval' 'approval_requests_require_an_owned_in_flight_turn_and_bounded_input'
    Invoke-Test 'lazoo-store' 'approval' 'system_closure_and_restart_reconciliation_fail_closed'
    $evidence.ledgerRevisionsAndIdempotency = $true

    Invoke-Test 'lazood' 'daemon' 'ask_turn_waits_for_the_user_decision_and_forwards_approval_to_the_provider'
    $evidence.askBlocksProviderUntilUserDecision = $true

    Invoke-Test 'lazood' 'daemon' 'declined_and_expired_approvals_fail_closed_at_the_provider'
    Invoke-Test 'lazood' 'daemon' 'cancelling_a_turn_with_a_pending_approval_declines_the_provider_and_records_cancellation'
    $evidence.declineExpiryCancellationFailClosed = $true

    Invoke-Test 'lazoo-store' 'approval' 'policy_auto_approves_inside_scope_and_pends_scope_expansion'
    Invoke-Test 'lazood' 'daemon' 'self_decision_policy_auto_approves_in_scope_and_waits_on_scope_expansion'
    $evidence.selfDecisionScopePolicy = $true

    Invoke-Test 'lazood' 'daemon' 'pending_approvals_from_a_previous_boot_expire_with_owner_loss_on_restart'
    $evidence.restartOwnerLoss = $true

    Invoke-Test 'lazoo-providers' 'codex' 'codex_approval_requests_normalize_into_bounded_provider_neutral_requests'
    Invoke-Test 'lazoo-providers' 'codex' 'observed_codex_approval_requests_are_answered_with_the_lazoo_decision'
    $evidence.codexAdapterAnswersNativeRequest = $true

    Invoke-Test 'lazooctl' 'cli' 'approval_cli_maps_list_show_and_bound_decisions'
    Invoke-Test 'lazooctl' 'cli' 'approval_errors_use_exit_code_fourteen'
    $evidence.cliMappingAndExitCode = $true

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
        throw 'desktop UI changes are outside the approval gate slice'
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
            sessionWideGrants = 'ABSENT'
            otherProviderApprovals = 'FAIL_CLOSED'
        }
    }
    $manifest | ConvertTo-Json -Depth 8 | Set-Content -LiteralPath $manifestPath -Encoding utf8
}

Write-Host "PASS manifest: $manifestPath"
