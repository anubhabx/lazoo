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
    $OutputPath = Join-Path $artifactDir 'collaboration-tools-manifest.json'
}
$manifestPath = [IO.Path]::GetFullPath($OutputPath)
$manifestDirectory = Split-Path -Parent $manifestPath
New-Item -ItemType Directory -Force -Path $manifestDirectory | Out-Null

$startedAt = [DateTime]::UtcNow
$stopwatch = [Diagnostics.Stopwatch]::StartNew()
$commands = [Collections.Generic.List[object]]::new()
$manifestStatus = 'FAIL'
$evidence = [ordered]@{
    unattachedObserverFailsClosed = $false
    codexAdapterAnswersNativeCall = $false
    experimentalApiOnlyWithTools = $false
    resumeWithToolsFailsClosed = $false
    toolsDeclaredForCodexSessions = $false
    leadDelegatesAndWaits = $false
    askDelegationProposed = $false
    refusalsNeverEndTurn = $false
    workerReportsThroughTool = $false
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
    Invoke-Test 'lazoo-providers' 'contract' 'observer_tool_calls_fail_closed_without_an_attached_control_plane'
    $evidence.unattachedObserverFailsClosed = $true

    Invoke-Test 'lazoo-providers' 'codex' 'codex_tool_calls_normalize_with_the_active_turn_fallback'
    Invoke-Test 'lazoo-providers' 'codex' 'observed_codex_tool_calls_are_answered_with_the_lazoo_result'
    $evidence.codexAdapterAnswersNativeCall = $true

    Invoke-Test 'lazoo-providers' 'codex' 'codex_sessions_without_tools_stay_stable_and_resume_with_tools_fails_closed'
    $evidence.experimentalApiOnlyWithTools = $true
    $evidence.resumeWithToolsFailsClosed = $true

    Invoke-Test 'lazood' 'daemon' 'lead_turn_lists_employees_delegates_and_waits_through_collaboration_tools'
    $evidence.toolsDeclaredForCodexSessions = $true
    $evidence.leadDelegatesAndWaits = $true

    Invoke-Test 'lazood' 'daemon' 'ask_lead_delegation_through_tools_is_proposed_and_refusals_never_end_the_turn'
    $evidence.askDelegationProposed = $true
    $evidence.refusalsNeverEndTurn = $true

    Invoke-Test 'lazood' 'daemon' 'worker_turn_reports_failure_through_task_report_before_native_completion'
    $evidence.workerReportsThroughTool = $true

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
        throw 'desktop UI changes are outside the collaboration tools slice'
    }
    $evidence.uiChangesAbsent = $true
    $manifestStatus = 'PASS'
} finally {
    Pop-Location
    $stopwatch.Stop()
    $manifest = [ordered]@{
        status = $manifestStatus
        schemaVersion = 18
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
            otherProviderTools = 'NOT_DECLARED'
            peerMessagingTools = 'DEFERRED'
        }
    }
    $manifest | ConvertTo-Json -Depth 8 | Set-Content -LiteralPath $manifestPath -Encoding utf8
}

Write-Host "PASS manifest: $manifestPath"
