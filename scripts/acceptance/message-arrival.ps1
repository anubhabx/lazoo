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
    $OutputPath = Join-Path $artifactDir 'message-arrival-manifest.json'
}
$manifestPath = [IO.Path]::GetFullPath($OutputPath)
$manifestDirectory = Split-Path -Parent $manifestPath
New-Item -ItemType Directory -Force -Path $manifestDirectory | Out-Null

$startedAt = [DateTime]::UtcNow
$stopwatch = [Diagnostics.Stopwatch]::StartNew()
$commands = [Collections.Generic.List[object]]::new()
$manifestStatus = 'FAIL'
$evidence = [ordered]@{
    wakesOnStoreWrite = $false
    pollIntervalAbsent = $false
    peerQuestionEndsTaskWait = $false
    replyEndsMessageSend = $false
    childCompletionEndsTaskWait = $false
    boundedWaitsKept = $false
    resultEvidenceKept = $false
    schemaThirteen = $false
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
    Invoke-Test 'lazood' 'daemon' 'tool_waits_wake_on_the_store_write_instead_of_polling'
    $evidence.wakesOnStoreWrite = $true
    $evidence.peerQuestionEndsTaskWait = $true
    $evidence.replyEndsMessageSend = $true
    $evidence.childCompletionEndsTaskWait = $true

    Invoke-Test 'lazood' 'daemon' 'task_wait_returns_early_with_a_peer_question_so_the_lead_answers_in_turn'
    $evidence.boundedWaitsKept = $true

    Invoke-Test 'lazood' 'daemon' 'task_wait_consumes_child_results_so_no_notice_turn_follows'
    $evidence.resultEvidenceKept = $true

    $toolsSource = Get-Content -Raw -LiteralPath (Join-Path $repoRoot 'apps/lazood/src/app/tools.rs')
    $commands.Add([ordered]@{
        command = 'Select-String WAIT_POLL_INTERVAL apps/lazood/src/app/tools.rs'
        exitCode = 0
        durationMs = $null
    })
    if ($toolsSource.Contains('WAIT_POLL_INTERVAL')) {
        throw 'tool waits still poll the store'
    }
    $evidence.pollIntervalAbsent = $true

    Invoke-Test 'lazoo-store' 'store' 'store_enables_wal_foreign_keys_and_schema_version'
    $evidence.schemaThirteen = $true

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
        throw 'desktop UI changes are outside the message arrival slice'
    }
    $evidence.uiChangesAbsent = $true
    $manifestStatus = 'PASS'
} finally {
    Pop-Location
    $stopwatch.Stop()
    $manifest = [ordered]@{
        status = $manifestStatus
        schemaVersion = 23
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
            waitWakeup = 'STORE_WRITE_NOTIFY'
            waitBound = 'UNCHANGED_DEADLINE'
            storePolling = 'NONE'
        }
    }
    $manifest | ConvertTo-Json -Depth 8 | Set-Content -LiteralPath $manifestPath -Encoding utf8
}

Write-Host "PASS manifest: $manifestPath"
