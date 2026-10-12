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
    $OutputPath = Join-Path $artifactDir 'workspace-conversation-manifest.json'
}
$manifestPath = [IO.Path]::GetFullPath($OutputPath)
$manifestDirectory = Split-Path -Parent $manifestPath
New-Item -ItemType Directory -Force -Path $manifestDirectory | Out-Null

$startedAt = [DateTime]::UtcNow
$stopwatch = [Diagnostics.Stopwatch]::StartNew()
$commands = [Collections.Generic.List[object]]::new()
$manifestStatus = 'FAIL'
$evidence = [ordered]@{
    lifecycleFollowsTheSession = $false
    workerCardsFollowTheRows = $false
    cliMapsWorkspaceShow = $false
    daemonServesTheConversation = $false
    headerCarriesAuthorityAndSession = $false
    attentionIsEmbedded = $false
    turnsAreBounded = $false
    marksResumeWithoutLoss = $false
    workspaceStoresNothing = $false
    snapshotUnchanged = $false
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
    Invoke-Test 'lazoo-domain' 'workspace' 'the_lead_lifecycle_follows_the_session_and_its_newest_turn'
    $evidence.lifecycleFollowsTheSession = $true
    Invoke-Test 'lazoo-domain' 'workspace' 'worker_cards_take_the_first_state_that_holds_with_lazoo_words'
    $evidence.workerCardsFollowTheRows = $true

    Invoke-Test 'lazooctl' 'cli' 'workspace_cli_maps_show'
    $evidence.cliMapsWorkspaceShow = $true

    Invoke-Test 'lazood' 'daemon' 'the_user_opens_the_conversation_header_and_worker_dock_through_the_daemon'
    $evidence.daemonServesTheConversation = $true
    $evidence.headerCarriesAuthorityAndSession = $true
    $evidence.attentionIsEmbedded = $true
    $evidence.turnsAreBounded = $true
    Invoke-Test 'lazood' 'daemon' 'the_desktop_resumes_every_read_model_from_its_event_high_water_without_loss'
    $evidence.marksResumeWithoutLoss = $true
    $evidence.workspaceStoresNothing = $true
    $evidence.snapshotUnchanged = $true

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
        throw 'desktop UI changes are outside the workspace conversation slice'
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
            storage = 'DERIVED_NEVER_STORED'
            mutations = 'NONE'
            phase = 'TWO_HEADER_AND_WORKER_DOCK'
            deferred = 'WORKSPACE_ACTIONS_AS_DATA'
            toolAccess = 'NONE'
        }
    }
    $manifest | ConvertTo-Json -Depth 8 | Set-Content -LiteralPath $manifestPath -Encoding utf8
}

Write-Host "PASS manifest: $manifestPath"
