param(
    [switch]$SkipBuild,
    [string]$OutputPath = ''
)

# Owner terminal attach to a suspended employee session, against the real daemon and CLI with the
# turn-transport fixture standing in for codex.exe. No live provider and no model call: the resume
# command itself (Claude `--resume <id>`, `codex resume <id>`) is a separate owner-authorized gate.

$ErrorActionPreference = 'Stop'
Set-StrictMode -Version Latest

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

function Invoke-Captured([string[]]$Arguments) {
    $previousRoot = $env:LAZOO_DATA_ROOT
    $previousPreference = $ErrorActionPreference
    try {
        $env:LAZOO_DATA_ROOT = $script:dataRoot
        $ErrorActionPreference = 'Continue'
        $timer = [Diagnostics.Stopwatch]::StartNew()
        $captured = @(& $script:ctlExe --json @Arguments 2>&1)
        $exitCode = $LASTEXITCODE
        $timer.Stop()
        $script:commands.Add([ordered]@{
            command = "lazooctl --json $($Arguments -join ' ')"
            exitCode = $exitCode
            durationMs = [long]$timer.ElapsedMilliseconds
        })
        return [pscustomobject]@{
            ExitCode = $exitCode
            Text = ($captured -join [Environment]::NewLine)
        }
    } finally {
        $ErrorActionPreference = $previousPreference
        if ($null -eq $previousRoot) {
            Remove-Item Env:LAZOO_DATA_ROOT -ErrorAction SilentlyContinue
        } else {
            $env:LAZOO_DATA_ROOT = $previousRoot
        }
    }
}

function Invoke-Json([string[]]$Arguments) {
    $result = Invoke-Captured $Arguments
    if ($result.ExitCode -ne 0) {
        throw "lazooctl $($Arguments -join ' ') exited $($result.ExitCode) with: $($result.Text)"
    }
    return ($result.Text | ConvertFrom-Json)
}

function Invoke-ExpectedFailure([int]$ExpectedExit, [string]$ExpectedCode, [string[]]$Arguments) {
    $result = Invoke-Captured $Arguments
    Assert-Equal $result.ExitCode $ExpectedExit "exit code of: $($Arguments -join ' ')"
    Assert-True $result.Text.Contains($ExpectedCode) "$ExpectedCode in: $($result.Text)"
    return $result.Text
}

function Start-Daemon {
    $startInfo = [Diagnostics.ProcessStartInfo]::new()
    $startInfo.FileName = $script:daemonExe
    $startInfo.UseShellExecute = $false
    $startInfo.CreateNoWindow = $true
    $startInfo.Environment['LAZOO_DATA_ROOT'] = $script:dataRoot
    foreach ($outerCodexVariable in @('CODEX_THREAD_ID', 'CODEX_SESSION_ID')) {
        [void]$startInfo.Environment.Remove($outerCodexVariable)
    }
    $startInfo.Environment['PATH'] = "$script:fixtureDir;$env:PATH"
    $startInfo.Environment['LAZOO_TURN_FIXTURE_LOG'] = $script:fixtureLog
    $startInfo.Environment['LAZOO_TURN_FIXTURE_THREAD_ID'] = $script:threadId
    $process = [Diagnostics.Process]::new()
    $process.StartInfo = $startInfo
    Assert-True $process.Start() 'daemon process start'
    return $process
}

function Wait-Status([int]$TimeoutSeconds = 20) {
    $deadline = [DateTime]::UtcNow.AddSeconds($TimeoutSeconds)
    $lastError = ''
    while ([DateTime]::UtcNow -lt $deadline) {
        try {
            return Invoke-Json @('status')
        } catch {
            $lastError = $_.Exception.Message
        }
        Start-Sleep -Milliseconds 100
    }
    throw "daemon did not become ready: $lastError"
}

function Stop-Daemon([Diagnostics.Process]$Process) {
    Invoke-Json @('system', 'shutdown') | Out-Null
    if (-not $Process.WaitForExit(15000)) {
        throw "daemon $($Process.Id) did not stop after planned shutdown"
    }
    Assert-Equal $Process.ExitCode 0 'planned daemon exit code'
}

function Get-FixtureStarts {
    if (-not (Test-Path -LiteralPath $script:fixtureLog)) {
        return 0
    }
    return @(Get-Content -LiteralPath $script:fixtureLog | Where-Object { $_ -eq 'server-start' }).Count
}

function Wait-FixtureStarts([int]$Expected, [int]$TimeoutSeconds = 10) {
    $deadline = [DateTime]::UtcNow.AddSeconds($TimeoutSeconds)
    while ([DateTime]::UtcNow -lt $deadline) {
        if ((Get-FixtureStarts) -ge $Expected) {
            return
        }
        Start-Sleep -Milliseconds 100
    }
    throw "fixture log did not reach $Expected server-start entries"
}

function Wait-TerminalOutput([string]$TerminalId, [string]$BootId, [string]$Needle, [int]$TimeoutSeconds = 10) {
    $deadline = [DateTime]::UtcNow.AddSeconds($TimeoutSeconds)
    $after = [uint64]0
    $accumulated = ''
    while ([DateTime]::UtcNow -lt $deadline) {
        $read = Invoke-Json @('terminal', 'read', $TerminalId, '--expected-boot-id', $BootId, '--after', ([string]$after), '--limit', '65536')
        $bytes = [byte[]]@($read.bytes | ForEach-Object { [byte]$_ })
        if ($bytes.Length -gt 0) {
            $accumulated += [Text.Encoding]::UTF8.GetString($bytes)
        }
        $after = [uint64]$read.output.end
        if ($accumulated.Contains($Needle)) {
            return $accumulated
        }
        Start-Sleep -Milliseconds 150
    }
    throw "terminal output did not contain '$Needle' within $TimeoutSeconds seconds; saw $($accumulated.Length) characters"
}

$repoRoot = (Resolve-Path (Join-Path $PSScriptRoot '..\..')).Path
$cargoExe = (Get-Command cargo -CommandType Application -ErrorAction Stop | Select-Object -First 1).Source
$gitExe = (Get-Command git -CommandType Application -ErrorAction Stop | Select-Object -First 1).Source
$releaseDir = Join-Path $repoRoot 'target\release'
$script:daemonExe = Join-Path $releaseDir 'lazood.exe'
$script:ctlExe = Join-Path $releaseDir 'lazooctl.exe'
# The daemon spawns every provider host through this sibling helper; it must sit beside lazood.exe.
$sessionHostExe = Join-Path $releaseDir 'lazoo-session-host.exe'
$fixtureExe = Join-Path $releaseDir 'lazoo-turn-transport-fixture.exe'
$artifactDir = Join-Path $repoRoot 'artifacts\acceptance'
if ([string]::IsNullOrWhiteSpace($OutputPath)) {
    $OutputPath = Join-Path $artifactDir 'session-terminal-attach-manifest.json'
}
$manifestPath = [IO.Path]::GetFullPath($OutputPath)
New-Item -ItemType Directory -Force -Path (Split-Path -Parent $manifestPath) | Out-Null

$runId = [Guid]::NewGuid().ToString('N')
$tempBase = [IO.Path]::GetFullPath([IO.Path]::GetTempPath())
$runRoot = [IO.Path]::GetFullPath((Join-Path $tempBase "lazoo-session-terminal-attach-$runId"))
if (-not $runRoot.StartsWith($tempBase, [StringComparison]::OrdinalIgnoreCase)) {
    throw 'acceptance run root is outside the system temporary directory'
}
$script:dataRoot = Join-Path $runRoot 'data'
$script:fixtureDir = Join-Path $runRoot 'fixture-bin'
$script:fixtureLog = Join-Path $runRoot 'fixture.log'
$script:threadId = "fixture-thread-$runId"
$startedAt = [DateTime]::UtcNow
$stopwatch = [Diagnostics.Stopwatch]::StartNew()
$script:commands = [Collections.Generic.List[object]]::new()
$daemon = $null
$manifestStatus = 'FAIL'
$evidence = [ordered]@{
    schemaTwenty = $false
    idleAttachRefused = $false
    resumeArgumentsFromRecords = $false
    cwdFromRecords = $false
    attachedTerminalLive = $false
    attachedProcessIsTheDiscoveredExecutable = $false
    oneLiveTerminalPerSession = $false
    terminalBytesRoundTrip = $false
    reattachAfterStop = $false
    sessionRecordUnchanged = $false
    unknownSessionRefused = $false
    auditEventRecorded = $false
    providerSmokeAbsent = $true
    externalModelUseAbsent = $true
    uiChangesAbsent = $false
}
$details = [ordered]@{}

New-Item -ItemType Directory -Force -Path $script:dataRoot, $script:fixtureDir | Out-Null

try {
    if (-not $SkipBuild) {
        $timer = [Diagnostics.Stopwatch]::StartNew()
        & $cargoExe build -p lazood -p lazoo-session-host -p lazooctl -p lazoo-turn-transport-fixture --release --locked
        $buildExit = $LASTEXITCODE
        $timer.Stop()
        $script:commands.Add([ordered]@{
            command = 'cargo build -p lazood -p lazoo-session-host -p lazooctl -p lazoo-turn-transport-fixture --release --locked'
            exitCode = $buildExit
            durationMs = [long]$timer.ElapsedMilliseconds
        })
        Assert-Equal $buildExit 0 'locked release build'
    }
    foreach ($binary in @($script:daemonExe, $sessionHostExe, $script:ctlExe, $fixtureExe)) {
        Assert-True (Test-Path -LiteralPath $binary) "binary exists: $binary"
    }
    Copy-Item -LiteralPath $fixtureExe -Destination (Join-Path $script:fixtureDir 'codex.exe')

    $daemon = Start-Daemon
    $status = Wait-Status
    Assert-Equal ([int]$status.schemaVersion) 23 'schema 23'
    $evidence.schemaTwenty = $true

    $employee = Invoke-Json @(
        'employee', 'create', 'Rhea', '--provider', 'codex', '--role', 'Lead',
        '--idempotency-key', "employee-$runId"
    )
    $employeeId = [string]$employee.employee.id
    $warmed = Invoke-Json @(
        'session', 'warm', $employeeId, '--expected-employee-revision', '1',
        '--idempotency-key', "warm-$runId"
    )
    Assert-Equal ([string]$warmed.session.state) 'idle' 'warmed session state'
    Assert-Equal ([string]$warmed.session.nativeSessionId) $script:threadId 'native session id from the fixture'
    $sessionId = [string]$warmed.session.id
    $details.sessionId = $sessionId

    Invoke-ExpectedFailure 10 'LZ-SESSION-STATE' @(
        'session', 'attach', $sessionId, '--expected-revision', ([string]$warmed.session.revision),
        '--idempotency-key', "attach-idle-$runId"
    ) | Out-Null
    $evidence.idleAttachRefused = $true

    $stopped = Invoke-Json @(
        'session', 'stop', $sessionId, '--expected-revision', ([string]$warmed.session.revision),
        '--idempotency-key', "stop-$runId"
    )
    Assert-Equal ([string]$stopped.session.state) 'suspended' 'stopped session state'
    $suspendedRevision = [long]$stopped.session.revision
    $startsBeforeAttach = Get-FixtureStarts
    Assert-True ($startsBeforeAttach -ge 1) 'the warm session ran the fixture as its protocol host'

    $attached = Invoke-Json @(
        'session', 'attach', $sessionId, '--expected-revision', ([string]$suspendedRevision),
        '--idempotency-key', "attach-$runId"
    )
    Assert-Equal ([string]$attached.sessionId) $sessionId 'attach names the session'
    $terminal = $attached.terminal
    $terminalId = [string]$terminal.id
    $bootId = [string]$terminal.bootId
    $details.terminalId = $terminalId
    $details.executable = [string]$terminal.executable
    Assert-True ([string]$terminal.executable).EndsWith('\fixture-bin\codex.exe', [StringComparison]::OrdinalIgnoreCase) 'attach runs the discovered codex executable'
    Assert-Equal @($terminal.args).Count 2 'attach argument count'
    Assert-Equal ([string]$terminal.args[0]) 'resume' 'attach resume verb'
    Assert-Equal ([string]$terminal.args[1]) $script:threadId 'attach resumes the recorded native session id'
    $evidence.resumeArgumentsFromRecords = $true
    Assert-Equal ([string]$terminal.cwd) (Join-Path $script:dataRoot "sessions\$sessionId") 'attach cwd is the session working directory'
    $evidence.cwdFromRecords = $true
    Assert-Equal ([bool]$terminal.live) $true 'attached terminal is live'
    $evidence.attachedTerminalLive = $true
    Wait-FixtureStarts ($startsBeforeAttach + 1)
    $evidence.attachedProcessIsTheDiscoveredExecutable = $true

    $second = Invoke-ExpectedFailure 10 'LZ-SESSION-ATTACHED' @(
        'session', 'attach', $sessionId, '--expected-revision', ([string]$suspendedRevision),
        '--idempotency-key', "attach-second-$runId"
    )
    Assert-True $second.Contains($terminalId) 'the refusal names the live terminal'
    $evidence.oneLiveTerminalPerSession = $true

    Invoke-Json @(
        'terminal', 'write', $terminalId, '--expected-boot-id', $bootId,
        '--text', ('{"id":1,"method":"initialize"}' + "`r`n")
    ) | Out-Null
    $output = Wait-TerminalOutput $terminalId $bootId 'lazoo-turn-transport-fixture'
    $details.terminalOutputChars = $output.Length
    $evidence.terminalBytesRoundTrip = $true

    $stoppedTerminal = Invoke-Json @(
        'terminal', 'stop', $terminalId, '--expected-revision', ([string]$terminal.revision),
        '--expected-boot-id', $bootId
    )
    Assert-Equal ([bool]$stoppedTerminal.live) $false 'stopped terminal is not live'
    $reattached = Invoke-Json @(
        'session', 'attach', $sessionId, '--expected-revision', ([string]$suspendedRevision),
        '--idempotency-key', "attach-again-$runId"
    )
    $reattachedId = [string]$reattached.terminal.id
    Assert-True ($reattachedId -ne $terminalId) 're-attach opens a new terminal'
    Assert-Equal ([bool]$reattached.terminal.live) $true 're-attached terminal is live'
    $details.reattachedTerminalId = $reattachedId
    Invoke-Json @(
        'terminal', 'stop', $reattachedId, '--expected-revision', ([string]$reattached.terminal.revision),
        '--expected-boot-id', $bootId
    ) | Out-Null
    $evidence.reattachAfterStop = $true

    $shown = Invoke-Json @('session', 'show', $sessionId)
    Assert-Equal ([string]$shown.state) 'suspended' 'session stays suspended'
    Assert-Equal ([long]$shown.revision) $suspendedRevision 'session revision unchanged by attach'
    Assert-Equal ([string]$shown.nativeSessionId) $script:threadId 'native session id retained'
    $evidence.sessionRecordUnchanged = $true

    Invoke-ExpectedFailure 10 'LZ-SESSION-NOT-FOUND' @(
        'session', 'attach', ([Guid]::NewGuid().ToString()), '--expected-revision', '1',
        '--idempotency-key', "attach-unknown-$runId"
    ) | Out-Null
    $evidence.unknownSessionRefused = $true

    $events = @(Invoke-Json @('events', 'list', '--after', '0', '--limit', '1024'))
    $attachEvents = @($events | Where-Object { $_.eventType -eq 'session.terminal.attached' })
    Assert-Equal $attachEvents.Count 2 'one audit event per attach'
    Assert-Equal ([string]$attachEvents[0].payload.sessionId) $sessionId 'audit event session'
    Assert-Equal ([string]$attachEvents[0].payload.terminalId) $terminalId 'audit event terminal'
    Assert-Equal ([string]$attachEvents[1].payload.terminalId) $reattachedId 'second audit event terminal'
    $evidence.auditEventRecorded = $true

    Stop-Daemon $daemon
    $daemon = $null

    & $gitExe diff --quiet -- apps/lazoo-desktop
    $unstagedExit = $LASTEXITCODE
    & $gitExe diff --cached --quiet -- apps/lazoo-desktop
    $stagedExit = $LASTEXITCODE
    $script:commands.Add([ordered]@{
        command = 'git diff --quiet and git diff --cached --quiet -- apps/lazoo-desktop'
        exitCode = [Math]::Max($unstagedExit, $stagedExit)
        durationMs = $null
    })
    if (($unstagedExit -ne 0) -or ($stagedExit -ne 0)) {
        throw 'desktop UI changes are outside the session terminal attach slice'
    }
    $evidence.uiChangesAbsent = $true
    $manifestStatus = 'PASS'
} catch {
    $details.failure = $_.Exception.Message
    throw
} finally {
    $stopwatch.Stop()
    if ($null -ne $daemon -and -not $daemon.HasExited) {
        Stop-Process -Id $daemon.Id -Force
        Wait-Process -Id $daemon.Id -Timeout 5 -ErrorAction SilentlyContinue
    }
    $manifest = [ordered]@{
        status = $manifestStatus
        schemaVersion = 23
        mode = 'real-binaries-disposable-data-root-no-provider'
        startedAt = $startedAt.ToString('o')
        durationMs = [long]$stopwatch.ElapsedMilliseconds
        isolatedDataRoot = $script:dataRoot
        commands = @($script:commands)
        evidence = $evidence
        details = $details
        boundaries = [ordered]@{
            providerSmoke = 'NOT_RUN'
            externalModelUse = 'NOT_RUN'
            liveNativeResume = 'NOT_PROVEN_OWNER_GATE'
            attachableStates = 'SUSPENDED_ONLY'
            attachableProviders = 'CODEX_CLAUDE_CODE_OPENCODE_ANTIGRAVITY'
            terminalAuthority = 'OWNER_NATIVE_CONFIGURATION'
            employeeActivationWhileAttached = 'ALLOWED'
            desktopUi = 'UNCHANGED'
            schemaMigration = 'NONE'
        }
    }
    $manifest | ConvertTo-Json -Depth 8 | Set-Content -LiteralPath $manifestPath -Encoding utf8
}

Write-Host "PASS manifest: $manifestPath"
