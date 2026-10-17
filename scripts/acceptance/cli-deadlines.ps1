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
    $OutputPath = Join-Path $artifactDir 'cli-deadlines-manifest.json'
}
$manifestPath = [IO.Path]::GetFullPath($OutputPath)
$manifestDirectory = Split-Path -Parent $manifestPath
New-Item -ItemType Directory -Force -Path $manifestDirectory | Out-Null

$startedAt = [DateTime]::UtcNow
$stopwatch = [Diagnostics.Stopwatch]::StartNew()
$commands = [Collections.Generic.List[object]]::new()
$manifestStatus = 'FAIL'
$evidence = [ordered]@{
    stalledDaemonResponseIsLabelledIpcTimeoutExitThree = $false
    readOnlyQueriesShortTierUnderHarnessKill = $false
    otherRpcsLongTierAboveProviderTurn = $false
    governorInitLockTimesOutInsteadOfWaitingForever = $false
    providerSmokeAbsent = $true
    externalModelUseAbsent = $true
    uiChangesAbsent = $false
}

# $Target is a tests/<name>.rs integration binary, or '--lib' for a crate's unit tests.
function Invoke-Test([string]$Package, [string]$Target, [string]$Name) {
    $targetArguments = if ($Target -eq '--lib') { @('--lib') } else { @('--test', $Target) }
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
    Invoke-Test 'lazooctl' '--lib' 'tests::stalled_daemon_response_times_out_cleanly'
    $evidence.stalledDaemonResponseIsLabelledIpcTimeoutExitThree = $true

    Invoke-Test 'lazooctl' '--lib' 'tests::response_timeout_tiers_read_only_queries_short_and_everything_else_long'
    $evidence.readOnlyQueriesShortTierUnderHarnessKill = $true
    $evidence.otherRpcsLongTierAboveProviderTurn = $true

    Invoke-Test 'lazoo-runtime' 'governor' 'initialization_lock_held_elsewhere_times_out_in_child_helper'
    $evidence.governorInitLockTimesOutInsteadOfWaitingForever = $true

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
        throw 'desktop UI changes are outside the CLI deadlines slice'
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
            schemaMigration = 'NONE'
            errorCode = 'LZ-IPC_EXIT_3_UNCHANGED'
            requestWriteDeadline = '5s'
            readOnlyResponseDeadline = '20s'
            otherResponseDeadline = '600s'
            eventsFollow = 'UNBOUNDED_BY_DESIGN'
            governorInitializationLock = '5s_THEN_RUN_UNENROLLED'
            cliStressTimeoutCause = 'NOT_ESTABLISHED'
            heapCorruptionExit = 'NOT_FIXED_NO_FIRST_PARTY_DEFECT_FOUND'
        }
    }
    $manifest | ConvertTo-Json -Depth 8 | Set-Content -LiteralPath $manifestPath -Encoding utf8
}

Write-Host "PASS manifest: $manifestPath"
