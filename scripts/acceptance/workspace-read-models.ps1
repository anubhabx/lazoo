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
    $OutputPath = Join-Path $artifactDir 'workspace-read-models-manifest.json'
}
$manifestPath = [IO.Path]::GetFullPath($OutputPath)
$manifestDirectory = Split-Path -Parent $manifestPath
New-Item -ItemType Directory -Force -Path $manifestDirectory | Out-Null

$startedAt = [DateTime]::UtcNow
$stopwatch = [Diagnostics.Stopwatch]::StartNew()
$commands = [Collections.Generic.List[object]]::new()
$manifestStatus = 'FAIL'
$evidence = [ordered]@{
    markersFollowTheRows = $false
    titlesDeriveFromTheFirstRequest = $false
    startabilityFollowsAvailability = $false
    cliMapsWorkspaceCommands = $false
    daemonServesSpaces = $false
    rememberedLeadIsTheNewest = $false
    launcherExplainsBlocks = $false
    pagesWithCursor = $false
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
    Invoke-Test 'lazoo-domain' 'workspace' 'the_marker_prefers_attention_then_activity_then_failure_then_completion'
    $evidence.markersFollowTheRows = $true
    Invoke-Test 'lazoo-domain' 'workspace' 'titles_come_from_the_store_then_the_first_request_then_a_default'
    $evidence.titlesDeriveFromTheFirstRequest = $true
    Invoke-Test 'lazoo-domain' 'workspace' 'startability_follows_roster_availability_with_lazoo_words_for_each_block'
    $evidence.startabilityFollowsAvailability = $true

    Invoke-Test 'lazooctl' 'cli' 'workspace_cli_maps_conversations_and_launcher'
    $evidence.cliMapsWorkspaceCommands = $true

    Invoke-Test 'lazood' 'daemon' 'the_user_browses_a_space_and_its_launcher_through_the_daemon'
    $evidence.daemonServesSpaces = $true
    $evidence.rememberedLeadIsTheNewest = $true
    $evidence.launcherExplainsBlocks = $true
    $evidence.pagesWithCursor = $true
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
        throw 'desktop UI changes are outside the workspace read models slice'
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
            schemaMigration = 'NONE'
            storage = 'DERIVED_NEVER_STORED'
            mutations = 'NONE'
            phase = 'ONE_SIDEBAR_AND_LAUNCHER'
            deferred = 'WORKSPACE_CONVERSATION_HEADER_AND_WORKER_DOCK'
            toolAccess = 'NONE'
        }
    }
    $manifest | ConvertTo-Json -Depth 8 | Set-Content -LiteralPath $manifestPath -Encoding utf8
}

Write-Host "PASS manifest: $manifestPath"
