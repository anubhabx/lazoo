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
    $OutputPath = Join-Path $artifactDir 'automatic-task-dispatch-manifest.json'
}
$manifestPath = [IO.Path]::GetFullPath($OutputPath)
$manifestDirectory = Split-Path -Parent $manifestPath
New-Item -ItemType Directory -Force -Path $manifestDirectory | Out-Null

$startedAt = [DateTime]::UtcNow
$stopwatch = [Diagnostics.Stopwatch]::StartNew()
$commands = [Collections.Generic.List[object]]::new()
$manifestStatus = 'FAIL'
$evidence = [ordered]@{
    backgroundWakeup = $false
    canonicalMessage = $false
    projectlessWorkspace = $false
    gitWorktreeIsolation = $false
    preProviderFailureAtomic = $false
    restartFailClosed = $false
    inFlightRestartUncertain = $false
    cancellationAtomic = $false
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
    Invoke-Test 'lazood' 'daemon' 'accepted_projectless_child_dispatches_automatically_through_the_canonical_message'
    $evidence.backgroundWakeup = $true
    $evidence.canonicalMessage = $true
    $evidence.projectlessWorkspace = $true

    Invoke-Test 'lazood' 'daemon' 'project_task_dispatch_uses_an_isolated_worktree_and_preserves_dirty_user_files'
    $evidence.gitWorktreeIsolation = $true

    Invoke-Test 'lazood' 'daemon' 'automatic_dispatch_configuration_drift_terminalizes_before_provider_io'
    $evidence.preProviderFailureAtomic = $true

    Invoke-Test 'lazoo-store' 'task' 'stale_pre_provider_dispatch_terminalizes_the_task_and_canonical_message_on_restart'
    $evidence.restartFailClosed = $true

    Invoke-Test 'lazoo-store' 'task' 'accepted_child_dispatch_claim_is_durable_exclusive_and_idempotent'
    $evidence.inFlightRestartUncertain = $true

    Invoke-Test 'lazoo-store' 'task' 'cancelling_a_claimed_dispatch_cancels_the_canonical_message_atomically'
    $evidence.cancellationAtomic = $true

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
        throw 'desktop UI changes are outside the automatic dispatch slice'
    }
    $evidence.uiChangesAbsent = $true
    $manifestStatus = 'PASS'
} finally {
    Pop-Location
    $stopwatch.Stop()
    $manifest = [ordered]@{
        status = $manifestStatus
        schemaVersion = 12
        mode = 'deterministic-fake-transport'
        startedAt = $startedAt.ToString('o')
        durationMs = [long]$stopwatch.ElapsedMilliseconds
        commands = @($commands)
        evidence = $evidence
        boundaries = [ordered]@{
            providerSmoke = 'NOT_RUN'
            externalModelUse = 'NOT_RUN'
            desktopUi = 'UNCHANGED'
            taskWorktreeCleanup = 'RETAINED_FOR_AUDIT'
        }
    }
    $manifest | ConvertTo-Json -Depth 8 | Set-Content -LiteralPath $manifestPath -Encoding utf8
}

Write-Host "PASS manifest: $manifestPath"
