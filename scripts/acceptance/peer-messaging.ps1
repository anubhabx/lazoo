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
    $OutputPath = Join-Path $artifactDir 'peer-messaging-manifest.json'
}
$manifestPath = [IO.Path]::GetFullPath($OutputPath)
$manifestDirectory = Split-Path -Parent $manifestPath
New-Item -ItemType Directory -Force -Path $manifestDirectory | Out-Null

$startedAt = [DateTime]::UtcNow
$stopwatch = [Diagnostics.Stopwatch]::StartNew()
$commands = [Collections.Generic.List[object]]::new()
$manifestStatus = 'FAIL'
$evidence = [ordered]@{
    peerTextBoundedAndTruthful = $false
    peerMessagesLinkBothTasks = $false
    repliesThreadUnderTheQuestion = $false
    inTurnConsumptionWithEvidence = $false
    staleNoticeTurnClosed = $false
    peerTurnNeverCompletesTask = $false
    idleLeadReceivesPeerTurn = $false
    waitingWorkerReceivesReplyInline = $false
    taskWaitReturnsEarlyForPeerMessages = $false
    dormantRecipientNeverActivated = $false
    userResumeDeliversPeerTurn = $false
    refusalsNeverEndTurn = $false
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
    Invoke-Test 'lazoo-domain' 'delivery' 'peer_message_text_names_the_sender_tasks_and_reply_tool'
    Invoke-Test 'lazoo-domain' 'delivery' 'peer_reply_text_threads_under_the_question'
    Invoke-Test 'lazoo-domain' 'delivery' 'peer_message_content_is_refused_when_empty_or_oversized'
    $evidence.peerTextBoundedAndTruthful = $true

    Invoke-Test 'lazoo-store' 'delivery' 'peer_messages_link_both_tasks_and_queue_once'
    $evidence.peerMessagesLinkBothTasks = $true

    Invoke-Test 'lazoo-store' 'delivery' 'replies_thread_under_the_question_and_return_to_the_sender_task'
    $evidence.repliesThreadUnderTheQuestion = $true

    Invoke-Test 'lazoo-store' 'delivery' 'peer_messages_are_consumed_in_turn_with_evidence_and_close_an_orphan_notice_turn'
    $evidence.inTurnConsumptionWithEvidence = $true
    $evidence.staleNoticeTurnClosed = $true

    Invoke-Test 'lazoo-store' 'delivery' 'peer_turns_never_complete_the_recipient_task'
    $evidence.peerTurnNeverCompletesTask = $true

    Invoke-Test 'lazood' 'daemon' 'peer_questions_reach_an_idle_lead_as_a_turn_and_the_reply_returns_to_the_waiting_worker'
    $evidence.idleLeadReceivesPeerTurn = $true
    $evidence.waitingWorkerReceivesReplyInline = $true

    Invoke-Test 'lazood' 'daemon' 'task_wait_returns_early_with_a_peer_question_so_the_lead_answers_in_turn'
    $evidence.taskWaitReturnsEarlyForPeerMessages = $true

    Invoke-Test 'lazood' 'daemon' 'peer_messages_never_activate_a_dormant_recipient_and_refusals_never_end_the_turn'
    $evidence.dormantRecipientNeverActivated = $true
    $evidence.userResumeDeliversPeerTurn = $true
    $evidence.refusalsNeverEndTurn = $true

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
        throw 'desktop UI changes are outside the peer messaging slice'
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
            schemaMigration = 'NONE'
            automaticRecipientActivation = 'NEVER'
            crossTreeMessaging = 'REFUSED'
            coalescedNotices = 'DEFERRED'
            otherProviderTools = 'NOT_DECLARED'
        }
    }
    $manifest | ConvertTo-Json -Depth 8 | Set-Content -LiteralPath $manifestPath -Encoding utf8
}

Write-Host "PASS manifest: $manifestPath"
