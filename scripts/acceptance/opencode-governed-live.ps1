param(
    [string]$ReleaseDir = '',
    [int]$TurnTimeoutSeconds = 240,
    [int]$ApprovalWaitSeconds = 150
)

$ErrorActionPreference = 'Stop'
Set-StrictMode -Version Latest

# LIVE acceptance of OpenCode governed delivery (PROJECT-BRIEF sections 12 and 21.3): one daemon
# lifetime on a disposable data root and, on the owner's real OpenCode install and free model,
# the approval ledger exercised end to end through the real `opencode acp` permission bridge.
# It sends real prompts, so it is never part of scripts/gate.ps1 (the gate's $excluded list must
# name it, exactly as it names provider-live-lifecycle). It never reads or writes the owner's
# OpenCode configuration: the daemon launches `opencode acp` with an inline OPENCODE_PERMISSION
# block chosen by the employee's authority.
#
# Exactly FOUR live turns, no retries:
#   smoke   `provider smoke opencode` verifies the provider in this data root (the probe cannot
#           see model readiness), as provider-live-lifecycle does.
#   turn 1  an OpenCode lead under `ask` is asked to run `hostname`. The script polls
#           `inbox list` until an `action-required` approval appears, records it (requesting
#           employee, action, scope), requires kind `command-execution` and the fixed action
#           text, approves it with `approval approve`, and requires the turn to complete with the
#           machine name.
#   turn 2  the same prompt; the approval is declined with `approval decline`. The approval must
#           end `declined` and the turn must record at least one `toolUpdate` whose final status
#           is `failed` and none that completed.
#   turn 3  a second OpenCode employee under `self-decision` takes the same prompt. The ACP
#           bridge marks every request a scope expansion (no portable confinement proof), so the
#           policy is expected to leave it for the user: the script approves what appears. Either
#           way the turn must have an approved approval record decided by the user or by policy,
#           and the run fails if none exists. The turn must complete with the machine name.
# Then a planned shutdown; the native process tree must be gone.
#
# The integrator rebuilds target\release first (`cargo build --workspace --release --locked`) and
# runs this from the main checkout, or passes -ReleaseDir. Parse-check only: do not run it where
# a real model turn is not wanted.

$repoRoot = (Resolve-Path (Join-Path $PSScriptRoot '..\..')).Path
if ([string]::IsNullOrWhiteSpace($ReleaseDir)) {
    $ReleaseDir = Join-Path $repoRoot 'target\release'
}
$script:daemonExe = Join-Path $ReleaseDir 'lazood.exe'
$script:ctlExe = Join-Path $ReleaseDir 'lazooctl.exe'
$artifactDir = Join-Path $repoRoot 'artifacts\acceptance'
$manifestPath = Join-Path $artifactDir 'opencode-governed-live-manifest.json'
$script:runId = [Guid]::NewGuid().ToString('N')
$tempBase = [IO.Path]::GetFullPath([IO.Path]::GetTempPath())
$script:runRoot = [IO.Path]::GetFullPath((Join-Path $tempBase "lazoo-live-opencode-governed-$script:runId"))
if (-not $script:runRoot.StartsWith($tempBase, [StringComparison]::OrdinalIgnoreCase)) {
    throw 'acceptance run root is outside the system temporary directory'
}
$script:dataRoot = Join-Path $script:runRoot 'data'
$startedAt = [DateTime]::UtcNow
$stopwatch = [Diagnostics.Stopwatch]::StartNew()
$script:commands = [Collections.Generic.List[object]]::new()
$script:createdPids = [Collections.Generic.List[int]]::new()
$script:scrubbedEnvironment = [Collections.Generic.List[string]]::new()
$script:turnDurations = [ordered]@{}
$script:polls = 0
$script:liveTurns = 0
$script:outcome = 'FAILED_ASSERTION'
# One smoke turn plus three governed turns.
$maxLiveTurns = 4
# A value the model cannot know without running a command (the free model answered the earlier
# `echo LAZOO_ASK_OK` prompt from the prompt text, raising no permission request at all).
$askPrompt = 'Use your shell tool to run the command hostname and reply with exactly its output and nothing else. Do not guess it.'
$okToken = [string]$env:COMPUTERNAME
# The outer agent session variables are scrubbed so the only channel left is the native install.
$scrubPattern = '^(CLAUDECODE$|CLAUDE_CODE_(?!GIT_BASH_PATH$)|CLAUDE_PID$|CLAUDE_EFFORT$|CLAUDE_PLUGIN_|CODEX_THREAD_ID$|CODEX_SESSION_ID$|AI_AGENT$|OPENCODE_PERMISSION$|OPENCODE_CONFIG$|OPENCODE_CONFIG_CONTENT$|OPENCODE_CONFIG_DIR$)'
$gates = [ordered]@{
    verification = 'NOT_RUN'
    askApproved = 'NOT_RUN'
    askDeclined = 'NOT_RUN'
    selfDecision = 'NOT_RUN'
    plannedShutdown = 'NOT_RUN'
}
$currentGate = 'verification'
$script:evidence = [ordered]@{}
$evidence = $script:evidence
$turns = [ordered]@{}
$daemon = $null
$manifestStatus = 'FAIL'
$failureMessage = ''
$providerVersion = ''

function Assert-Equal($Actual, $Expected, [string]$Label) {
    if ($Actual -ne $Expected) {
        throw "$Label expected '$Expected' but found '$Actual'"
    }
}

function Assert-True([bool]$Condition, [string]$Label) {
    if (-not $Condition) {
        throw "$Label expected true"
    }
}

function ConvertTo-SafeText([string]$Message, [int]$Limit = 600) {
    if ([string]::IsNullOrWhiteSpace($Message)) { return '' }
    $safe = $Message -replace '(?i)(api[_-]?key|token|authorization|secret|password|bearer)\s*[:=]?\s*\S+', '$1=<redacted>'
    $safe = $safe -replace '[A-Z0-9._%+-]+@[A-Z0-9.-]+\.[A-Z]{2,}', '<redacted-email>'
    if ($safe.Length -gt $Limit) { $safe = $safe.Substring(0, $Limit) }
    return $safe
}

function Set-DataRoot([string]$Value) {
    $previous = $env:LAZOO_DATA_ROOT
    $env:LAZOO_DATA_ROOT = $Value
    return $previous
}

function Restore-DataRoot($Previous) {
    if ($null -eq $Previous) {
        Remove-Item Env:LAZOO_DATA_ROOT -ErrorAction SilentlyContinue
    } else {
        $env:LAZOO_DATA_ROOT = $Previous
    }
}

# Runs lazooctl --json and returns the exit code and text. Polling calls pass -Quiet so the manifest
# keeps the commands that matter and a poll count instead of hundreds of identical lines.
function Invoke-Ctl([string[]]$Arguments, [switch]$Quiet) {
    $previousRoot = Set-DataRoot $script:dataRoot
    $previousPreference = $ErrorActionPreference
    try {
        $ErrorActionPreference = 'Continue'
        $timer = [Diagnostics.Stopwatch]::StartNew()
        $captured = @(& $script:ctlExe --json @Arguments 2>&1)
        $exitCode = $LASTEXITCODE
        $timer.Stop()
        if ($Quiet) {
            $script:polls++
        } else {
            $script:commands.Add([ordered]@{
                command = "lazooctl --json $($Arguments -join ' ')"
                exitCode = $exitCode
                durationMs = $timer.ElapsedMilliseconds
            })
        }
        return [pscustomobject]@{ ExitCode = $exitCode; Text = ($captured -join [Environment]::NewLine) }
    } finally {
        $ErrorActionPreference = $previousPreference
        Restore-DataRoot $previousRoot
    }
}

function Invoke-Json([string[]]$Arguments, [switch]$Quiet) {
    $result = Invoke-Ctl $Arguments -Quiet:$Quiet
    if ($result.ExitCode -ne 0) {
        throw "lazooctl $($Arguments -join ' ') exited $($result.ExitCode) with: $(ConvertTo-SafeText $result.Text)"
    }
    return ($result.Text | ConvertFrom-Json)
}

function Start-Daemon {
    $startInfo = [Diagnostics.ProcessStartInfo]::new()
    $startInfo.FileName = $script:daemonExe
    $startInfo.UseShellExecute = $false
    $startInfo.CreateNoWindow = $true
    $startInfo.Environment['LAZOO_DATA_ROOT'] = $script:dataRoot
    # Only the names are kept; values are never read into the manifest. A pre-set
    # OPENCODE_PERMISSION would reach OpenCode under every mode, and OPENCODE_CONFIG,
    # OPENCODE_CONFIG_CONTENT, and OPENCODE_CONFIG_DIR can carry permission rules the inline block
    # does not outrank (the provider also refuses to start a governed session beside them), so
    # all four are scrubbed. XDG variables are kept: the owner's authentication stays reachable.
    foreach ($name in @($startInfo.Environment.Keys)) {
        if ($name -match $scrubPattern) {
            [void]$startInfo.Environment.Remove($name)
            $script:scrubbedEnvironment.Add([string]$name)
        }
    }
    $process = [Diagnostics.Process]::new()
    $process.StartInfo = $startInfo
    Assert-True $process.Start() 'daemon process start'
    $script:createdPids.Add($process.Id)
    return $process
}

function Wait-Status([int]$TimeoutSeconds = 30) {
    $deadline = [DateTime]::UtcNow.AddSeconds($TimeoutSeconds)
    $lastError = ''
    while ([DateTime]::UtcNow -lt $deadline) {
        $result = Invoke-Ctl @('status') -Quiet
        if ($result.ExitCode -eq 0) { return ($result.Text | ConvertFrom-Json) }
        $lastError = $result.Text
        Start-Sleep -Milliseconds 200
    }
    throw "daemon did not become ready: $(ConvertTo-SafeText $lastError)"
}

# A blocking lazooctl call run in the background so the script can act while a turn is in flight.
function Start-BackgroundCtl([string]$Name, [string[]]$Arguments) {
    $stdoutPath = Join-Path $script:runRoot "$Name.stdout.json"
    $stderrPath = Join-Path $script:runRoot "$Name.stderr.txt"
    $previousRoot = Set-DataRoot $script:dataRoot
    try {
        # Start-Process joins its arguments with spaces, so each one is quoted here.
        $quoted = @(@('--json') + $Arguments | ForEach-Object { '"' + (([string]$_) -replace '"', '\"') + '"' })
        $process = Start-Process -FilePath $script:ctlExe -ArgumentList $quoted `
            -RedirectStandardOutput $stdoutPath -RedirectStandardError $stderrPath `
            -PassThru -WindowStyle Hidden
        $script:createdPids.Add($process.Id)
        return [pscustomobject]@{
            Name = $Name
            Process = $process
            StdoutPath = $stdoutPath
            StderrPath = $stderrPath
            Arguments = $Arguments
            Timer = [Diagnostics.Stopwatch]::StartNew()
        }
    } finally {
        Restore-DataRoot $previousRoot
    }
}

# Waits for a background call; a timeout is a recorded outcome, never a hang.
function Wait-BackgroundCtl($Command, [int]$TimeoutSeconds) {
    $finished = $Command.Process.WaitForExit($TimeoutSeconds * 1000)
    $Command.Timer.Stop()
    if (-not $finished) {
        $script:commands.Add([ordered]@{
            command = "lazooctl --json $($Command.Arguments -join ' ')"
            exitCode = $null
            durationMs = $Command.Timer.ElapsedMilliseconds
        })
        return [pscustomobject]@{ TimedOut = $true; ExitCode = $null; Value = $null; Text = ''; DurationMs = $Command.Timer.ElapsedMilliseconds }
    }
    $Command.Process.WaitForExit()
    $Command.Process.Refresh()
    $exitCode = [int]$Command.Process.ExitCode
    $stdout = if (Test-Path -LiteralPath $Command.StdoutPath) { Get-Content -LiteralPath $Command.StdoutPath -Raw } else { '' }
    $stderr = if (Test-Path -LiteralPath $Command.StderrPath) { Get-Content -LiteralPath $Command.StderrPath -Raw } else { '' }
    if ($null -eq $stdout) { $stdout = '' }
    if ($null -eq $stderr) { $stderr = '' }
    $script:commands.Add([ordered]@{
        command = "lazooctl --json $($Command.Arguments -join ' ')"
        exitCode = $exitCode
        durationMs = $Command.Timer.ElapsedMilliseconds
    })
    $value = $null
    if ($exitCode -eq 0 -and $stdout.Trim() -ne '') { $value = $stdout | ConvertFrom-Json }
    return [pscustomobject]@{
        TimedOut = $false; ExitCode = $exitCode; Value = $value
        Text = (ConvertTo-SafeText ($stderr + ' ' + $stdout)); DurationMs = $Command.Timer.ElapsedMilliseconds
    }
}

function Test-AuthenticationText([string]$Text) {
    return ($Text -match '(?i)auth|log ?in|logged|credential|unauthori[sz]ed|401|subscription|expired|api key')
}

# Reads a property that may be absent from a JSON object without tripping strict mode.
function Get-Field($Object, [string]$Name) {
    if ($null -eq $Object) { return $null }
    $property = $Object.PSObject.Properties[$Name]
    if ($null -eq $property) { return $null }
    return $property.Value
}

function Limit-Output([string]$Text) {
    if ($null -eq $Text) { return $null }
    $trimmed = $Text.Trim()
    if ($trimmed.Length -gt 400) { return $trimmed.Substring(0, 400) }
    return $trimmed
}

function Get-TurnOutput($TurnResult) {
    $completed = @($TurnResult.turn.events | Where-Object { $_.kind -eq 'turnCompleted' })
    if ($completed.Count -eq 0) { return $null }
    return [string]$completed[-1].payload.output
}

# The status of every tool call the turn recorded. A command that ran has a completed one.
function Get-ToolStatuses($TurnResult) {
    return @(@($TurnResult.turn.events | Where-Object { $_.kind -eq 'toolUpdate' }) | ForEach-Object {
        [string](Get-Field $_.payload 'status')
    })
}

function Submit-LiveTurn([string]$Name, [string]$ConversationId, [string]$Authority) {
    if ($script:liveTurns -ge $maxLiveTurns) { throw "refusing a live turn beyond the cap of $maxLiveTurns" }
    $script:liveTurns++
    return (Start-BackgroundCtl "$Name-submit" @(
        'turn', 'submit', $ConversationId, '--authority-mode', $Authority,
        '--text', $askPrompt, '--activate-if-needed', '--idempotency-key', "$Name-$script:runId"))
}

# Polls the Inbox until an action-required approval for the employee appears, or the turn ends
# first. Returns the item, or $null when the turn finished without ever asking.
function Wait-ForApproval($TurnCommand, [string]$EmployeeId) {
    $deadline = [DateTime]::UtcNow.AddSeconds($ApprovalWaitSeconds)
    while ([DateTime]::UtcNow -lt $deadline) {
        $listed = Invoke-Json @('inbox', 'list', '--filter', 'all', '--limit', '50') -Quiet
        $items = @(@(Get-Field $listed 'items') | Where-Object {
            $_.kind -eq 'approval' -and $_.attention -eq 'action-required' -and
            [string]$_.scope.employeeId -eq $EmployeeId
        })
        if ($items.Count -gt 0) { return $items[0] }
        if ($TurnCommand.Process.HasExited) { return $null }
        Start-Sleep -Milliseconds 500
    }
    $script:outcome = 'NO_APPROVAL_WITHIN_WAIT'
    throw "no action-required approval appeared within $ApprovalWaitSeconds seconds"
}

function Format-Approval($Item) {
    return [ordered]@{
        id = [string]$Item.id
        revision = [int]$Item.revision
        subtype = [string]$Item.subtype
        title = ConvertTo-SafeText ([string]$Item.title) 200
        requiredAction = ConvertTo-SafeText ([string]$Item.requiredAction) 200
        requestingEmployee = [string]$Item.scope.employeeName
        provider = [string]$Item.scope.provider
        scope = [ordered]@{
            conversationId = [string]$Item.scope.conversationId
            turnId = [string]$Item.scope.turnId
            taskId = [string]$Item.scope.taskId
        }
        detail = ConvertTo-SafeText (($Item.detail | ConvertTo-Json -Compress -Depth 8)) 500
        actions = @(@($Item.actions) | ForEach-Object { [string]$_.action })
    }
}

# The approval record's kind and action text. The bridge admits only an execute request as a
# command-execution approval whose action is the fixed text below (no command or path target: the
# adapter drops titles as potentially secret).
function Assert-CommandApproval($Item, [string]$Label) {
    $record = Invoke-Json @('approval', 'show', [string]$Item.id)
    Assert-Equal ([string]$record.kind) 'command-execution' "$Label approval kind"
    Assert-Equal ([string]$record.action) 'execute an ACP command' "$Label approval action"
    return [ordered]@{ kind = [string]$record.kind; action = [string]$record.action; authority = [string]$record.authority; scopeExpansion = [bool]$record.scopeExpansion }
}

function Complete-LiveTurn($Command, [string]$Name) {
    $result = Wait-BackgroundCtl $Command $TurnTimeoutSeconds
    $script:turnDurations[$Name] = $result.DurationMs
    if ($result.TimedOut) {
        $script:outcome = 'TIMEOUT'
        throw "live turn '$Name' did not finish within $TurnTimeoutSeconds seconds"
    }
    if ($result.ExitCode -ne 0) {
        if ($result.Text -match 'LZ-(TURN-PROVIDER|TASK-AUTHORITY)-UNSUPPORTED') {
            # The daemon refused before any model was reached: it does not count as a live turn.
            $script:outcome = 'DELIVERY_REFUSED'
            $script:liveTurns--
            throw "the daemon refuses to deliver this turn to OpenCode: $($result.Text)"
        }
        $script:outcome = if (Test-AuthenticationText $result.Text) { 'BLOCKED_AUTHENTICATION' } else { 'TURN_FAILED' }
        throw "live turn '$Name' failed with lazooctl exit $($result.ExitCode): $($result.Text)"
    }
    return $result.Value
}

function Get-ProcessRecord([int]$ProcessId) {
    return Get-CimInstance Win32_Process -Filter "ProcessId = $ProcessId" -ErrorAction SilentlyContinue
}

function Get-ProcessTreeRecords([int]$RootProcessId) {
    $processes = @(Get-CimInstance Win32_Process)
    $records = [Collections.Generic.List[object]]::new()
    $pending = [Collections.Generic.Queue[int]]::new()
    $seen = [Collections.Generic.HashSet[int]]::new()
    $pending.Enqueue($RootProcessId)
    while ($pending.Count -gt 0) {
        $processId = $pending.Dequeue()
        if (-not $seen.Add($processId)) { continue }
        $record = $processes | Where-Object { $_.ProcessId -eq $processId } | Select-Object -First 1
        if ($null -ne $record) { $records.Add($record) }
        foreach ($child in @($processes | Where-Object { $_.ParentProcessId -eq $processId })) {
            $pending.Enqueue([int]$child.ProcessId)
        }
    }
    return @($records)
}

function Wait-ProcessGone([int]$ProcessId, [int]$TimeoutSeconds, [string]$Label) {
    $deadline = [DateTime]::UtcNow.AddSeconds($TimeoutSeconds)
    while ([DateTime]::UtcNow -lt $deadline) {
        if ($null -eq (Get-ProcessRecord $ProcessId)) { return }
        Start-Sleep -Milliseconds 100
    }
    throw "$Label process $ProcessId remained alive after $TimeoutSeconds seconds"
}

function New-Lead([string]$Name, [string]$Authority) {
    $employee = Invoke-Json @(
        'employee', 'create', $Name, '--provider', 'opencode', '--role', 'Lead',
        '--authority-mode', $Authority, '--idle-timeout-seconds', '0',
        '--idempotency-key', "lead-$Name-$script:runId")
    $employeeId = [string]$employee.employee.id
    $conversation = Invoke-Json @(
        'conversation', 'create', $employeeId, '--title', "OpenCode governed $Authority",
        '--idempotency-key', "conversation-$Name-$script:runId")
    return [pscustomobject]@{ EmployeeId = $employeeId; ConversationId = [string]$conversation.conversation.id }
}

New-Item -ItemType Directory -Force -Path $artifactDir, $script:dataRoot | Out-Null

try {
    foreach ($binary in @($script:daemonExe, $script:ctlExe)) {
        Assert-True (Test-Path -LiteralPath $binary) "binary exists: $binary"
    }
    $providerCommand = Get-Command opencode -CommandType Application -ErrorAction SilentlyContinue | Select-Object -First 1
    Assert-True ($null -ne $providerCommand) 'opencode executable on PATH'
    $providerVersion = ((& $providerCommand.Source --version 2>&1) -join ' ').Trim()

    $daemon = Start-Daemon
    $status = Wait-Status
    Assert-Equal $status.schemaVersion 21 'schema 21'

    # Verification: the probe cannot see model readiness (the free model needs no login), so one
    # live smoke turn verifies the provider first. It counts as a live turn.
    $probe = Invoke-Json @('provider', 'probe', 'opencode')
    $script:liveTurns++
    $smoke = Invoke-Json @('provider', 'smoke', 'opencode', '--prompt', 'Reply with exactly LAZOO_OPENCODE_OK. Do not use tools.')
    $smokeCompleted = @(@($smoke.events) | Where-Object { $_.type -eq 'turnCompleted' }) | Select-Object -Last 1
    Assert-True ($null -ne $smokeCompleted) 'verification smoke completed a turn'
    $evidence.verification = [ordered]@{
        probeVerification = [string](Get-Field $probe 'verification')
        smokeOutput = ConvertTo-SafeText ([string]$smokeCompleted.output)
        verificationAfterSmoke = [string](Get-Field (Invoke-Json @('provider', 'probe', 'opencode')) 'verification')
    }
    $gates.verification = 'PASS'

    # Turn 1: `ask`. The command waits for the user's one-action approval.
    $currentGate = 'askApproved'
    $opal = New-Lead 'Opal' 'ask'
    $first = Submit-LiveTurn 'turn1' $opal.ConversationId 'ask'
    $approval = Wait-ForApproval $first $opal.EmployeeId
    Assert-True ($null -ne $approval) 'turn 1 raised an action-required approval before finishing'
    $evidence.askApproval = Format-Approval $approval
    Assert-Equal ([string]$approval.state) 'pending' 'turn 1 approval state'
    $evidence.askApprovalRecord = Assert-CommandApproval $approval 'turn 1'
    Assert-True (-not $first.Process.HasExited) 'turn 1 waits for the decision'
    $decided = Invoke-Json @(
        'approval', 'approve', [string]$approval.id, '--expected-revision', [string]$approval.revision,
        '--idempotency-key', "approve-$script:runId")
    Assert-Equal ([string]$decided.approval.state) 'approved' 'turn 1 approval decision'
    Assert-Equal ([string]$decided.approval.decider) 'user' 'turn 1 approval decider'
    $firstTurn = Complete-LiveTurn $first 'turn1'
    $firstOutput = Get-TurnOutput $firstTurn
    $turns.askApproved = [ordered]@{
        prompt = $askPrompt
        state = [string]$firstTurn.turn.state
        output = Limit-Output $firstOutput
        toolStatuses = Get-ToolStatuses $firstTurn
        durationMs = $script:turnDurations['turn1']
    }
    Assert-Equal ([string]$firstTurn.turn.state) 'completed' 'turn 1 state'
    Assert-True ($null -ne $firstOutput -and $firstOutput -like "*$okToken*") "turn 1 output contains $okToken"
    $gates.askApproved = 'PASS'

    # Turn 2: the same prompt, declined. The decision is recorded and no command ran.
    $currentGate = 'askDeclined'
    $second = Submit-LiveTurn 'turn2' $opal.ConversationId 'ask'
    $approval = Wait-ForApproval $second $opal.EmployeeId
    Assert-True ($null -ne $approval) 'turn 2 raised an action-required approval before finishing'
    $evidence.declinedApproval = Format-Approval $approval
    $evidence.declinedApprovalRecord = Assert-CommandApproval $approval 'turn 2'
    $declined = Invoke-Json @(
        'approval', 'decline', [string]$approval.id, '--expected-revision', [string]$approval.revision,
        '--idempotency-key', "decline-$script:runId")
    Assert-Equal ([string]$declined.approval.state) 'declined' 'turn 2 approval decision'
    Assert-Equal ([string]$declined.approval.decider) 'user' 'turn 2 approval decider'
    $secondTurn = Complete-LiveTurn $second 'turn2'
    $secondStatuses = Get-ToolStatuses $secondTurn
    $turns.askDeclined = [ordered]@{
        prompt = $askPrompt
        state = [string]$secondTurn.turn.state
        output = Limit-Output (Get-TurnOutput $secondTurn)
        toolStatuses = $secondStatuses
        durationMs = $script:turnDurations['turn2']
    }
    $shown = Invoke-Json @('approval', 'show', [string]$approval.id)
    Assert-Equal ([string]$shown.state) 'declined' 'turn 2 approval stays declined'
    # The refused tool call must be recorded as failed, not merely absent as completed.
    Assert-True ($secondStatuses.Count -ge 1) 'turn 2 recorded at least one toolUpdate event'
    Assert-True ($secondStatuses -contains 'failed') 'turn 2 recorded a toolUpdate whose status is failed'
    Assert-Equal ([string]$secondStatuses[-1]) 'failed' 'turn 2 final toolUpdate status'
    Assert-True ($secondStatuses -notcontains 'completed') 'turn 2 recorded no completed tool call'
    $gates.askDeclined = 'PASS'

    # Turn 3: `self-decision` on a second employee. Record who resolved the request.
    $currentGate = 'selfDecision'
    $sven = New-Lead 'Sven' 'self-decision'
    $third = Submit-LiveTurn 'turn3' $sven.ConversationId 'self-decision'
    $approval = Wait-ForApproval $third $sven.EmployeeId
    if ($null -ne $approval) {
        $evidence.selfDecisionApproval = Format-Approval $approval
        $evidence.selfDecisionApprovalRecord = Assert-CommandApproval $approval 'turn 3'
        $decided = Invoke-Json @(
            'approval', 'approve', [string]$approval.id, '--expected-revision', [string]$approval.revision,
            '--idempotency-key', "approve-self-$script:runId")
        Assert-Equal ([string]$decided.approval.state) 'approved' 'turn 3 approval decision'
    }
    $thirdTurn = Complete-LiveTurn $third 'turn3'
    $thirdOutput = Get-TurnOutput $thirdTurn
    $turns.selfDecision = [ordered]@{
        prompt = $askPrompt
        state = [string]$thirdTurn.turn.state
        output = Limit-Output $thirdOutput
        toolStatuses = Get-ToolStatuses $thirdTurn
        durationMs = $script:turnDurations['turn3']
    }
    Assert-Equal ([string]$thirdTurn.turn.state) 'completed' 'turn 3 state'
    # Whichever way it was resolved, the turn must have an approval record, decided by the user or
    # by policy: a completed turn with no record would mean the command ran outside the ledger.
    $turnApprovals = @(Invoke-Json @('approval', 'list', '--turn-id', [string]$thirdTurn.turn.id, '--limit', '20'))
    Assert-True ($turnApprovals.Count -ge 1) 'turn 3 has an approval record'
    $decidedApprovals = @($turnApprovals | Where-Object {
        [string]$_.state -eq 'approved' -and ([string]$_.decider -in @('user', 'policy'))
    })
    Assert-True ($decidedApprovals.Count -ge 1) 'turn 3 has an approved record decided by the user or policy'
    $evidence.selfDecisionResolution = if ($null -ne $approval) { 'user-approved-scope-expansion' } else { 'policy-decided' }
    $evidence.selfDecisionRecords = @($turnApprovals | ForEach-Object {
        [ordered]@{ kind = [string]$_.kind; action = [string]$_.action; state = [string]$_.state; decider = [string]$_.decider; scopeExpansion = [bool]$_.scopeExpansion }
    })
    Assert-Equal ([string]$decidedApprovals[0].kind) 'command-execution' 'turn 3 approval kind'
    Assert-Equal ([string]$decidedApprovals[0].action) 'execute an ACP command' 'turn 3 approval action'
    Assert-True ($null -ne $thirdOutput -and $thirdOutput -like "*$okToken*") "turn 3 output contains $okToken"
    $gates.selfDecision = 'PASS'

    # Planned shutdown: the daemon's whole native tree must be gone afterwards.
    $currentGate = 'plannedShutdown'
    $shutdownTree = @(Get-ProcessTreeRecords $daemon.Id | Where-Object { $_.ProcessId -ne $daemon.Id })
    $evidence.shutdownTreeProcesses = $shutdownTree.Count
    Invoke-Json @('system', 'shutdown') | Out-Null
    Assert-True ($daemon.WaitForExit(20000)) 'daemon exited after planned shutdown'
    Assert-Equal $daemon.ExitCode 0 'planned daemon exit code'
    $daemon = $null
    foreach ($record in $shutdownTree) {
        Wait-ProcessGone ([int]$record.ProcessId) 15 'planned-shutdown native tree'
    }
    $gates.plannedShutdown = 'PASS'
    $manifestStatus = 'PASS'
    $script:outcome = 'PASS'
} catch {
    $failureMessage = ConvertTo-SafeText $_.Exception.Message 900
    $gates[$currentGate] = 'FAIL'
} finally {
    $stopwatch.Stop()
    if ($null -ne $daemon -and -not $daemon.HasExited) {
        Stop-Process -Id $daemon.Id -Force -ErrorAction SilentlyContinue
        Wait-Process -Id $daemon.Id -Timeout 5 -ErrorAction SilentlyContinue
    }
    foreach ($createdPid in $script:createdPids) {
        $created = Get-Process -Id $createdPid -ErrorAction SilentlyContinue
        if ($null -ne $created -and -not $created.HasExited) {
            Stop-Process -Id $createdPid -Force -ErrorAction SilentlyContinue
        }
    }
    $commandSnapshot = @($script:commands | ForEach-Object {
        [ordered]@{
            command = [string]$_.command
            exitCode = if ($null -eq $_.exitCode) { $null } else { [int]$_.exitCode }
            durationMs = if ($null -eq $_.durationMs) { $null } else { [long]$_.durationMs }
        }
    })
    $manifest = [ordered]@{
        manifestVersion = 1
        status = $manifestStatus
        outcome = $script:outcome
        failure = if ($failureMessage -eq '') { $null } else { $failureMessage }
        mode = 'live-native-install'
        schemaVersion = 21
        startedAt = $startedAt.ToString('o')
        completedAt = [DateTime]::UtcNow.ToString('o')
        durationMs = $stopwatch.ElapsedMilliseconds
        provider = 'opencode'
        providerVersion = $providerVersion
        scrubbedEnvironmentVariableNames = @($script:scrubbedEnvironment | Sort-Object -Unique)
        liveTurnsSent = $script:liveTurns
        liveTurnCap = $maxLiveTurns
        turnTimeoutSeconds = $TurnTimeoutSeconds
        approvalWaitSeconds = $ApprovalWaitSeconds
        isolatedDataRoot = $script:dataRoot
        temporaryDataRetained = $false
        prompt = $askPrompt
        turns = $turns
        turnDurationsMs = $script:turnDurations
        pollCalls = $script:polls
        commands = $commandSnapshot
        gates = $gates
        evidence = $evidence
        boundaries = [ordered]@{
            permissions = 'EXERCISED_ASK_APPROVE_DECLINE_AND_SELF_DECISION'
            deterministicGate = 'EXCLUDED'
            userProviderConfiguration = 'UNTOUCHED'
            networkAndConnectorKinds = 'DECLINED_NOT_GOVERNED_NOT_EXERCISED_LIVE'
        }
    }
    $manifest | ConvertTo-Json -Depth 12 | Set-Content -LiteralPath $manifestPath -Encoding UTF8
    $resolvedRunRoot = [IO.Path]::GetFullPath($script:runRoot)
    if ($resolvedRunRoot.StartsWith($tempBase, [StringComparison]::OrdinalIgnoreCase) -and
        (Split-Path -Leaf $resolvedRunRoot).StartsWith('lazoo-live-')) {
        Remove-Item -LiteralPath $resolvedRunRoot -Recurse -Force -ErrorAction SilentlyContinue
    }
}

if ($manifestStatus -ne 'PASS') {
    Write-Output "FAIL ($script:outcome) at gate '$currentGate': $failureMessage"
    Write-Output "manifest: $manifestPath"
    exit 1
}
Write-Output "PASS manifest: $manifestPath"
