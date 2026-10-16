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
    $OutputPath = Join-Path $artifactDir 'raw-frame-archive-manifest.json'
}
$manifestPath = [IO.Path]::GetFullPath($OutputPath)
$manifestDirectory = Split-Path -Parent $manifestPath
New-Item -ItemType Directory -Force -Path $manifestDirectory | Out-Null

$startedAt = [DateTime]::UtcNow
$stopwatch = [Diagnostics.Stopwatch]::StartNew()
$commands = [Collections.Generic.List[object]]::new()
$manifestStatus = 'FAIL'
$evidence = [ordered]@{
    rawFramesArchived = $false
    redacted = $false
    cappedWithTrailer = $false
    quotaShared = $false
    cleanupCoversBothKinds = $false
    optOutWritesNothing = $false
    completedTurnsOnly = $false
    rawKindNeverSummarized = $false
    schemaEighteenUpgrades = $false
    schemaNineteen = $false
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
    Invoke-Test 'lazoo-store' 'memory' 'raw_frame_archive_is_hashed_capped_and_counted_against_quota'
    $evidence.cappedWithTrailer = $true
    $evidence.quotaShared = $true

    Invoke-Test 'lazoo-store' 'memory' 'raw_frame_archive_respects_retention_opt_out'
    $evidence.optOutWritesNothing = $true
    $evidence.completedTurnsOnly = $true

    Invoke-Test 'lazoo-store' 'memory' 'raw_frames_and_canonical_archive_coexist_and_cleanup_removes_both'
    $evidence.cleanupCoversBothKinds = $true
    $evidence.rawKindNeverSummarized = $true

    Invoke-Test 'lazoo-store' 'store' 'version_eighteen_archives_upgrade_as_canonical_turn_archives'
    $evidence.schemaEighteenUpgrades = $true

    Invoke-Test 'lazood' 'daemon' 'completed_turn_archives_redacted_raw_provider_frames'
    $evidence.rawFramesArchived = $true
    $evidence.redacted = $true

    Invoke-Test 'lazood' 'daemon' 'raw_frame_capture_is_skipped_when_retention_is_disabled'
    Invoke-Test 'lazood' 'daemon' 'raw_frame_capture_is_bounded_by_the_policy_cap'

    Invoke-Test 'lazoo-store' 'store' 'store_enables_wal_foreign_keys_and_schema_version'
    $evidence.schemaNineteen = $true

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
        throw 'desktop UI changes are outside the raw frame archive slice'
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
            schemaMigration = '0019_memory_raw_frames'
            providers = 'FAKE_TRANSPORT'
            capture = 'PER_MAPPED_PROVIDER_EVENT_RAW_PAYLOAD'
            wireFramesWithoutEvents = 'NOT_COVERED'
            nonCompletedTurns = 'NOT_COVERED'
            rawFrameSummarization = 'NONE'
            archiveListingRpc = 'NONE'
        }
    }
    $manifest | ConvertTo-Json -Depth 8 | Set-Content -LiteralPath $manifestPath -Encoding utf8
}

Write-Host "PASS manifest: $manifestPath"
