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
    $OutputPath = Join-Path $artifactDir 'opencode-governed-delivery-manifest.json'
}
$manifestPath = [IO.Path]::GetFullPath($OutputPath)
$manifestDirectory = Split-Path -Parent $manifestPath
New-Item -ItemType Directory -Force -Path $manifestDirectory | Out-Null

$startedAt = [DateTime]::UtcNow
$stopwatch = [Diagnostics.Stopwatch]::StartNew()
$commands = [Collections.Generic.List[object]]::new()
$manifestStatus = 'FAIL'
$evidence = [ordered]@{
    governedModesAskForEveryGovernedTool = $false
    governedConfigurationConflictRefused = $false
    refusalsAreRecordedAndNotProgress = $false
    launchPromptFreeOnlyUnderFullAutonomy = $false
    approvalLedgerGovernsAskAndSelfDecision = $false
    governedKindsMapped = $false
    unsupportedKindsDeclinedWithoutApproval = $false
    narrowerAuthorityDelivered = $false
    grokExcluded = $false
    memoryWorkerSupportsOpenCode = $false
    refusalsFailClosedBeforeIo = $false
    noSupportClaimed = $true
    schemaTwenty = $false
    providerSmokeAbsent = $true
    externalModelUseAbsent = $true
    uiChangesAbsent = $false
}

function Invoke-Test([string]$Package, [string]$Target, [string]$Name) {
    # An empty target runs the crate's unit tests.
    $targetArguments = if ($Target -eq '') { @('--lib') } else { @('--test', $Target) }
    $arguments = @('test', '-p', $Package) + $targetArguments + @($Name, '--locked', '--', '--exact')
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
    # The launch block per mode: every governed tool asks under ask and self-decision, and only
    # full autonomy is prompt-free.
    Invoke-Test 'lazoo-providers' 'opencode' 'opencode_launch_makes_every_governed_tool_ask_under_ask_and_self_decision'
    $evidence.governedModesAskForEveryGovernedTool = $true
    Invoke-Test 'lazoo-providers' 'opencode' 'opencode_launch_is_prompt_free_only_under_full_autonomy'
    $evidence.launchPromptFreeOnlyUnderFullAutonomy = $true

    # The ACP bridge: execute, edit, delete, and move reach the approval observer; every other
    # kind is answered with a cancel and a recorded reason, never a grant.
    Invoke-Test 'lazoo-providers' 'opencode' 'observed_acp_admits_command_edit_delete_and_move_as_governed_requests'
    Invoke-Test 'lazoo-providers' '' 'acp::tests::acp_delete_and_move_map_to_file_changes_and_every_other_kind_stays_refused'
    $evidence.governedKindsMapped = $true
    Invoke-Test 'lazoo-providers' 'opencode' 'observed_acp_refuses_every_other_kind_with_a_cancel_and_a_recorded_reason'

    # The daemon: the ledger over the fake backend and over the real adapter on the fake ACP
    # fixture; unsupported kinds never become approvals.
    Invoke-Test 'lazood' 'daemon' 'opencode_ask_and_self_decision_turns_are_governed_through_the_approval_ledger'
    $evidence.approvalLedgerGovernsAskAndSelfDecision = $true
    $evidence.unsupportedKindsDeclinedWithoutApproval = $true

    # Delivery by authority: OpenCode serves a narrower turn through its governed observer;
    # Grok Build remains excluded, and OpenCode may serve the read-only memory worker.
    Invoke-Test 'lazood' 'daemon' 'opencode_delivers_turns_and_tasks_under_governed_authority'
    $evidence.grokExcluded = $true
    $evidence.memoryWorkerSupportsOpenCode = $true
    Invoke-Test 'lazood' 'daemon' 'a_full_autonomy_opencode_employee_serves_a_narrower_delegated_task'
    $evidence.narrowerAuthorityDelivered = $true

    # Every governed launch gets the block; a project configuration that can override it is
    # refused; refusals are recorded and are not provider progress.
    Invoke-Test 'lazoo-providers' 'opencode' 'governed_sessions_refuse_a_project_configuration_that_can_override_the_block'
    Invoke-Test 'lazoo-providers' '' 'opencode::tests::governed_sessions_refuse_configuration_that_can_override_the_block'
    $evidence.governedConfigurationConflictRefused = $true
    Invoke-Test 'lazoo-providers' 'opencode' 'observed_acp_records_a_reused_jsonrpc_id_and_refusals_are_not_progress'
    $evidence.refusalsAreRecordedAndNotProgress = $true

    Invoke-Test 'lazood' 'daemon' 'task_turn_rejects_a_provider_without_authority_override_support_before_io'
    $evidence.refusalsFailClosedBeforeIo = $true

    Invoke-Test 'lazoo-store' 'store' 'store_enables_wal_foreign_keys_and_schema_version'
    $evidence.schemaTwenty = $true

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
        throw 'desktop UI changes are outside the current provider-parity contract'
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
            schemaMigration = 'CURRENT_20_NATIVE_WORKSPACE_TRUST'
            unlocked = 'OPENCODE_ASK_AND_SELF_DECISION_THROUGH_THE_APPROVAL_LEDGER'
            stillRefused = 'ACP_KINDS_OTHER_THAN_EXECUTE_EDIT_DELETE_MOVE_AND_GROK_BUILD'
            liveGate = 'NOT_RUN_NO_SUPPORT_CLAIMED'
            liveScript = 'scripts/acceptance/opencode-governed-live.ps1 (integrator; excluded from the gate)'
            toolAccess = 'GOVERNED_FOUR_PROVIDERS'
        }
    }
    $manifest | ConvertTo-Json -Depth 8 | Set-Content -LiteralPath $manifestPath -Encoding utf8
}

Write-Host "PASS manifest: $manifestPath"
