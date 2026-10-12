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
    $OutputPath = Join-Path $artifactDir 'desktop-inbox-manifest.json'
}
$manifestPath = [IO.Path]::GetFullPath($OutputPath)
$manifestDirectory = Split-Path -Parent $manifestPath
New-Item -ItemType Directory -Force -Path $manifestDirectory | Out-Null

$startedAt = [DateTime]::UtcNow
$stopwatch = [Diagnostics.Stopwatch]::StartNew()
$commands = [Collections.Generic.List[object]]::new()
$manifestStatus = 'FAIL'
$evidence = [ordered]@{
    inboxContractsStrict = $false
    everyKindDerivesFromItsRow = $false
    badgeCountsOnlyActionRequired = $false
    leadWindowWatchedThenCounted = $false
    acknowledgedVisibleUncounted = $false
    urgencyOrderStable = $false
    actionsNameExistingRpcs = $false
    archivePagesWithCursor = $false
    cliMapsInboxCommands = $false
    daemonServesInbox = $false
    inboxStoresNothing = $false
    toolsCannotSeeInbox = $false
    schemaFifteen = $false
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
    Invoke-Test 'lazoo-protocol' 'inbox' 'inbox_list_params_default_to_two_hundred_and_reject_unknown_fields'
    Invoke-Test 'lazoo-protocol' 'inbox' 'inbox_items_and_counts_serialize_with_camel_case_keys_and_kebab_case_values'
    $evidence.inboxContractsStrict = $true

    Invoke-Test 'lazoo-domain' 'inbox' 'a_pending_approval_needs_the_user_and_a_decided_one_is_the_archive'
    Invoke-Test 'lazoo-domain' 'inbox' 'proposed_and_uncertain_tasks_need_the_user_and_running_ones_are_not_items'
    Invoke-Test 'lazoo-domain' 'inbox' 'only_escalated_incidents_are_items_and_only_user_escalations_count'
    $evidence.everyKindDerivesFromItsRow = $true
    $evidence.actionsNameExistingRpcs = $true

    Invoke-Test 'lazoo-domain' 'inbox' 'counts_filters_and_order_follow_attention_then_deadline_then_age'
    Invoke-Test 'lazoo-domain' 'inbox' 'cursors_round_trip_and_limits_are_bounded'
    $evidence.badgeCountsOnlyActionRequired = $true
    $evidence.urgencyOrderStable = $true

    Invoke-Test 'lazooctl' 'cli' 'inbox_cli_maps_list_and_counts'
    $evidence.cliMapsInboxCommands = $true

    Invoke-Test 'lazood' 'daemon' 'the_inbox_lists_a_pending_approval_and_its_decision_archives_it'
    Invoke-Test 'lazood' 'daemon' 'the_inbox_derives_tasks_and_incidents_and_pages_the_archive'
    $evidence.daemonServesInbox = $true
    $evidence.leadWindowWatchedThenCounted = $true
    $evidence.acknowledgedVisibleUncounted = $true
    $evidence.archivePagesWithCursor = $true
    $evidence.inboxStoresNothing = $true
    $evidence.toolsCannotSeeInbox = $true

    Invoke-Test 'lazoo-store' 'store' 'store_enables_wal_foreign_keys_and_schema_version'
    $evidence.schemaFifteen = $true

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
        throw 'desktop UI changes are outside the desktop inbox slice'
    }
    $evidence.uiChangesAbsent = $true
    $manifestStatus = 'PASS'
} finally {
    Pop-Location
    $stopwatch.Stop()
    $manifest = [ordered]@{
        status = $manifestStatus
        schemaVersion = 15
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
            inboxStorage = 'DERIVED_NEVER_STORED'
            inboxMutations = 'NONE'
            actions = 'DATA_NAMING_EXISTING_RPCS'
            badge = 'ACTION_REQUIRED_ONLY'
            permissionsFilter = 'RESERVED_EMPTY_UNTIL_TERMINAL_PASSTHROUGH'
            toolAccess = 'NONE'
        }
    }
    $manifest | ConvertTo-Json -Depth 8 | Set-Content -LiteralPath $manifestPath -Encoding utf8
}

Write-Host "PASS manifest: $manifestPath"
