param(
    # Accepted for parity with the other acceptance scripts: the default already assumes the
    # release binaries and the packaged UI assets exist.
    [switch]$SkipBuild,
    # Build the UI assets (npm ci, npm run build) and the desktop binary before measuring.
    [switch]$Build,
    # Measure 5-minute idle and routing windows instead of the 60-second default windows.
    [switch]$FullWindows,
    [int]$WindowSeconds = 0,
    [int]$SettleSeconds = 20
)

$ErrorActionPreference = 'Stop'
Set-StrictMode -Version Latest

# Brief section 20.1, "Lazoo itself" including the desktop. This script measures the keeper,
# the daemon, the desktop process and every descendant of the desktop (the Tauri WebView2
# process tree) over per-second samples, and compares them with the idle and routing targets.
# It starts no provider and no model. Nothing is enforced.

function Assert-Equal($Actual, $Expected, [string]$Label) {
    if ($Actual -ne $Expected) { throw "$Label expected '$Expected' but found '$Actual'" }
}

function Assert-True([bool]$Condition, [string]$Label) {
    if (-not $Condition) { throw "$Label expected true" }
}

function Invoke-Json([string[]]$Arguments) {
    $previousRoot = $env:LAZOO_DATA_ROOT
    $previousPreference = $ErrorActionPreference
    try {
        $env:LAZOO_DATA_ROOT = $script:dataRoot
        $ErrorActionPreference = 'Continue'
        $commandTimer = [Diagnostics.Stopwatch]::StartNew()
        $captured = @(& $script:ctlExe --json @Arguments 2>&1)
        $exitCode = $LASTEXITCODE
        $commandTimer.Stop()
        if ($script:recordCommands) {
            $script:commands.Add([ordered]@{
                command = "lazooctl --json $($Arguments -join ' ')"
                exitCode = $exitCode
                durationMs = $commandTimer.ElapsedMilliseconds
            })
        }
        if ($exitCode -ne 0) {
            throw "lazooctl $($Arguments -join ' ') exited $exitCode with: $($captured -join [Environment]::NewLine)"
        }
        return (($captured -join [Environment]::NewLine) | ConvertFrom-Json)
    } finally {
        $ErrorActionPreference = $previousPreference
        $env:LAZOO_DATA_ROOT = $previousRoot
    }
}

function Wait-Status([int]$TimeoutSeconds = 20) {
    $deadline = [DateTime]::UtcNow.AddSeconds($TimeoutSeconds)
    $lastError = ''
    while ([DateTime]::UtcNow -lt $deadline) {
        try { return (Invoke-Json @('status')) } catch { $lastError = $_.Exception.Message }
        Start-Sleep -Milliseconds 100
    }
    throw "daemon did not become ready: $lastError"
}

function Start-Keeper([string]$DataRoot) {
    $startInfo = [Diagnostics.ProcessStartInfo]::new()
    $startInfo.FileName = $script:keeperExe
    $startInfo.UseShellExecute = $false
    $startInfo.CreateNoWindow = $true
    $startInfo.RedirectStandardOutput = $true
    $startInfo.RedirectStandardError = $true
    $startInfo.EnvironmentVariables['LAZOO_DATA_ROOT'] = $DataRoot
    $startInfo.EnvironmentVariables.Remove('LAZOO_DAEMON_PATH')
    $process = [Diagnostics.Process]::Start($startInfo)
    # Drain the pipes for the whole (possibly 10+ minute) run so the keeper can never block.
    $script:keeperStdoutTask = $process.StandardOutput.ReadToEndAsync()
    $script:keeperStderrTask = $process.StandardError.ReadToEndAsync()
    return $process
}

function Start-Desktop([string]$DataRoot) {
    $startInfo = [Diagnostics.ProcessStartInfo]::new()
    $startInfo.FileName = $script:desktopExe
    $startInfo.UseShellExecute = $false
    $startInfo.EnvironmentVariables['LAZOO_DATA_ROOT'] = $DataRoot
    return [Diagnostics.Process]::Start($startInfo)
}

function Get-ProcessTable {
    return @(Get-CimInstance Win32_Process -Property ProcessId, ParentProcessId, Name |
        Select-Object ProcessId, ParentProcessId, Name)
}

function Get-DescendantIds($Table, [int]$RootProcessId) {
    $children = @{}
    foreach ($row in $Table) {
        $parent = [int]$row.ParentProcessId
        if (-not $children.ContainsKey($parent)) { $children[$parent] = [Collections.Generic.List[int]]::new() }
        $children[$parent].Add([int]$row.ProcessId)
    }
    $result = [Collections.Generic.List[int]]::new()
    $seen = [Collections.Generic.HashSet[int]]::new()
    $queue = [Collections.Generic.Queue[int]]::new()
    $queue.Enqueue($RootProcessId)
    $seen.Add($RootProcessId) | Out-Null
    while ($queue.Count -gt 0) {
        $parent = $queue.Dequeue()
        if ($children.ContainsKey($parent)) {
            foreach ($child in $children[$parent]) {
                if ($seen.Add($child)) {
                    $result.Add($child)
                    $queue.Enqueue($child)
                }
            }
        }
    }
    return @($result)
}

function Stop-Tree([int]$RootProcessId) {
    for ($attempt = 0; $attempt -lt 20; $attempt += 1) {
        $ids = @(Get-DescendantIds (Get-ProcessTable) $RootProcessId)
        if ($ids.Count -eq 0) { break }
        foreach ($id in $ids) { Stop-Process -Id $id -Force -ErrorAction SilentlyContinue }
        Start-Sleep -Milliseconds 100
    }
    Stop-Process -Id $RootProcessId -Force -ErrorAction SilentlyContinue
}

function Wait-Window([Diagnostics.Process]$Process, [int]$TimeoutSeconds = 30) {
    $deadline = [DateTime]::UtcNow.AddSeconds($TimeoutSeconds)
    while ([DateTime]::UtcNow -lt $deadline) {
        if ($Process.HasExited) { throw "desktop exited early with code $($Process.ExitCode)" }
        $Process.Refresh()
        if ($Process.MainWindowHandle -ne [IntPtr]::Zero) { return $Process.MainWindowHandle }
        Start-Sleep -Milliseconds 100
    }
    throw 'desktop did not expose a main window handle'
}

function Get-AutomationNames([IntPtr]$WindowHandle) {
    $root = [System.Windows.Automation.AutomationElement]::FromHandle($WindowHandle)
    $elements = $root.FindAll(
        [System.Windows.Automation.TreeScope]::Descendants,
        [System.Windows.Automation.Condition]::TrueCondition
    )
    $names = [Collections.Generic.List[string]]::new()
    foreach ($item in $elements) {
        try {
            $name = $item.Current.Name
            if (-not [string]::IsNullOrWhiteSpace($name)) { $names.Add($name) }
        } catch { }
    }
    return @($names)
}

function Wait-UiText([IntPtr]$WindowHandle, [string[]]$Expected, [int]$TimeoutSeconds = 30) {
    $deadline = [DateTime]::UtcNow.AddSeconds($TimeoutSeconds)
    $lastNames = @()
    while ([DateTime]::UtcNow -lt $deadline) {
        try {
            $lastNames = @(Get-AutomationNames $WindowHandle)
            $missing = @($Expected | Where-Object { $_ -notin $lastNames })
            if ($missing.Count -eq 0) { return $lastNames }
        } catch { }
        Start-Sleep -Milliseconds 200
    }
    throw "desktop UI did not expose expected text '$($Expected -join ', ')'; observed: $($lastNames -join ' | ')"
}

# ---- Sampling: keeper, daemon, desktop and every desktop descendant ----------------------------

function Get-GroupOf([string]$Name, [int]$ProcessId, [bool]$UnderDesktop) {
    if ($ProcessId -eq $script:keeperPid) { return 'keeper' }
    if ($ProcessId -eq $script:daemonPid) { return 'daemon' }
    if ($ProcessId -eq $script:desktopPid) { return 'desktop' }
    if ($UnderDesktop -and $Name -eq 'msedgewebview2') { return 'webview2' }
    if ($UnderDesktop) { return 'desktop-other' }
    return 'keeper-other'
}

# Discover the Lazoo-owned process set. Newly seen processes start from zero when they started
# after the previous sample, and from their current CPU time otherwise (a baseline).
function Update-Roster {
    $table = Get-ProcessTable
    $desktopIds = [Collections.Generic.HashSet[int]]::new()
    foreach ($id in @(Get-DescendantIds $table $script:desktopPid)) { $desktopIds.Add($id) | Out-Null }
    $wanted = [Collections.Generic.Dictionary[int, bool]]::new()
    foreach ($id in @($script:keeperPid, $script:daemonPid, $script:desktopPid)) { $wanted[[int]$id] = ($id -eq $script:desktopPid) }
    foreach ($id in @(Get-DescendantIds $table $script:keeperPid)) { $wanted[[int]$id] = $false }
    foreach ($id in $desktopIds) { $wanted[[int]$id] = $true }
    foreach ($entry in $wanted.GetEnumerator()) {
        $processId = [int]$entry.Key
        if ($script:tracked.ContainsKey($processId)) { continue }
        try {
            $process = [Diagnostics.Process]::GetProcessById($processId)
            $name = $process.ProcessName
            $cpuNow = [long]$process.TotalProcessorTime.TotalMilliseconds
            $base = $cpuNow
            try {
                if ($script:hasSampled -and $process.StartTime.ToUniversalTime() -gt $script:lastSampleWall) { $base = 0 }
            } catch { }
            $script:tracked[$processId] = @{
                process = $process
                name = $name
                group = (Get-GroupOf $name $processId $entry.Value)
                lastCpuMs = $base
                alive = $true
            }
            $script:everSeen[$processId] = "$name"
        } catch {
            $script:unreadable["$processId"] = $_.Exception.Message
        }
    }
}

# One sample: CPU milliseconds consumed since the previous sample by every tracked process
# (including ones that exited in between, read from the kept handle), plus current memory.
function Get-ProcessSample {
    $wall = $script:clock.Elapsed.TotalMilliseconds
    $wallUtc = [DateTime]::UtcNow
    $cpuDelta = 0L
    $workingSet = 0L
    $privateBytes = 0L
    $alive = 0
    $groupCpu = @{}
    foreach ($processId in @($script:tracked.Keys)) {
        $item = $script:tracked[$processId]
        if (-not $item.alive) { continue }
        $process = $item.process
        try {
            $exited = $process.HasExited
            $cpuNow = [long]$process.TotalProcessorTime.TotalMilliseconds
            $delta = [Math]::Max(0L, $cpuNow - $item.lastCpuMs)
            $item.lastCpuMs = $cpuNow
            $cpuDelta += $delta
            if (-not $groupCpu.ContainsKey($item.group)) { $groupCpu[$item.group] = 0L }
            $groupCpu[$item.group] += $delta
            if ($exited) {
                $item.alive = $false
            } else {
                $process.Refresh()
                $workingSet += [long]$process.WorkingSet64
                $privateBytes += [long]$process.PrivateMemorySize64
                $alive += 1
            }
        } catch {
            $item.alive = $false
        }
    }
    $previousWall = $script:lastSampleWallMs
    $script:lastSampleWallMs = $wall
    $script:lastSampleWall = $wallUtc
    $script:hasSampled = $true
    return @{
        tMs = $wall
        wallMs = ($wall - $previousWall)
        cpuMs = $cpuDelta
        workingSetBytes = $workingSet
        privateBytes = $privateBytes
        processCount = $alive
        groupCpuMs = $groupCpu
    }
}

function Read-Sample([bool]$RefreshRoster) {
    if ($RefreshRoster) { Update-Roster }
    return (Get-ProcessSample)
}

function New-Window([string]$Name, [int]$Seconds) {
    return @{ name = $Name; seconds = $Seconds; samples = [Collections.Generic.List[object]]::new(); extra = [ordered]@{} }
}

function Get-RollingMaxShare($Samples, [double]$SpanMs) {
    $count = $Samples.Count
    if ($count -eq 0) { return $null }
    $total = 0.0
    foreach ($sample in $Samples) { $total += $sample.wallMs }
    if ($total -lt ($SpanMs * 0.98)) { return $null }
    $best = $null
    for ($end = 0; $end -lt $count; $end += 1) {
        $wallSum = 0.0
        $cpuSum = 0.0
        $reached = $false
        for ($index = $end; $index -ge 0; $index -= 1) {
            $wallSum += $Samples[$index].wallMs
            $cpuSum += $Samples[$index].cpuMs
            if ($wallSum -ge $SpanMs) { $reached = $true; break }
        }
        if (-not $reached) { continue }
        $share = $cpuSum / ($wallSum * $script:processorCount)
        if ($null -eq $best -or $share -gt $best) { $best = $share }
    }
    return $best
}

function Get-WindowSummary($Window) {
    $samples = $Window.samples
    $wallSum = 0.0
    $cpuSum = 0.0
    $peakShare = 0.0
    $wsSum = 0.0
    $wsPeak = 0L
    $privPeak = 0L
    $processPeak = 0
    $groupTotals = @{}
    foreach ($sample in $samples) {
        $wallSum += $sample.wallMs
        $cpuSum += $sample.cpuMs
        $share = $sample.cpuMs / ($sample.wallMs * $script:processorCount)
        if ($share -gt $peakShare) { $peakShare = $share }
        $wsSum += $sample.workingSetBytes
        if ($sample.workingSetBytes -gt $wsPeak) { $wsPeak = $sample.workingSetBytes }
        if ($sample.privateBytes -gt $privPeak) { $privPeak = $sample.privateBytes }
        if ($sample.processCount -gt $processPeak) { $processPeak = $sample.processCount }
        foreach ($key in $sample.groupCpuMs.Keys) {
            if (-not $groupTotals.ContainsKey($key)) { $groupTotals[$key] = 0L }
            $groupTotals[$key] += $sample.groupCpuMs[$key]
        }
    }
    $groups = [ordered]@{}
    foreach ($key in ($groupTotals.Keys | Sort-Object)) {
        $groups[$key] = [ordered]@{
            cpuMillis = [long]$groupTotals[$key]
            cpuShare = [double]$groupTotals[$key] / ($wallSum * $script:processorCount)
        }
    }
    $rolling30 = Get-RollingMaxShare $samples 30000
    $rolling300 = Get-RollingMaxShare $samples 300000
    $meanShare = if ($wallSum -gt 0) { $cpuSum / ($wallSum * $script:processorCount) } else { 0.0 }
    return [ordered]@{
        windowSeconds = $Window.seconds
        sampleCount = $samples.Count
        measuredWallMillis = [long]$wallSum
        totalCpuMillis = [long]$cpuSum
        cpuShareMean = $meanShare
        cpuShareMeanPeakSample1s = $peakShare
        cpuShareRollingMax30s = $rolling30
        cpuShareRollingMax5min = $rolling300
        rolling5minNote = if ($null -eq $rolling300) { 'not computed: window shorter than 300 seconds (use -FullWindows)' } else { 'computed' }
        workingSetBytesMean = [long]($wsSum / [Math]::Max(1, $samples.Count))
        workingSetBytesPeak = $wsPeak
        privateBytesPeak = $privPeak
        liveProcessCountPeak = $processPeak
        groups = $groups
        extra = $Window.extra
        samples = @($samples | ForEach-Object { , @([Math]::Round($_.tMs), [Math]::Round(($_.cpuMs / ($_.wallMs * $script:processorCount)), 5), $_.workingSetBytes) })
        samplesColumns = @('elapsedMs', 'cpuShare', 'workingSetBytes')
    }
}

# Sample every second for the window; $Activity (optional) runs between ticks.
function Invoke-Window($Window, [scriptblock]$Activity = $null) {
    $null = Read-Sample $true
    $deadline = $script:clock.Elapsed.TotalSeconds + $Window.seconds
    $nextTick = $script:clock.Elapsed.TotalMilliseconds + 1000
    $tickIndex = 0
    while ($script:clock.Elapsed.TotalSeconds -lt $deadline) {
        if ($null -ne $Activity) { & $Activity }
        $remaining = $nextTick - $script:clock.Elapsed.TotalMilliseconds
        if ($remaining -gt 0) {
            if ($null -eq $Activity) { Start-Sleep -Milliseconds ([int][Math]::Ceiling($remaining)) }
            continue
        }
        $tickIndex += 1
        $Window.samples.Add((Read-Sample (($tickIndex % 3) -eq 0)))
        $nextTick += 1000
        if ($nextTick -lt $script:clock.Elapsed.TotalMilliseconds) { $nextTick = $script:clock.Elapsed.TotalMilliseconds + 1000 }
    }
}

function Get-WorstShare($Summary) {
    $worst = [double]$Summary.cpuShareMean
    foreach ($value in @($Summary.cpuShareRollingMax30s, $Summary.cpuShareRollingMax5min)) {
        if ($null -ne $value -and [double]$value -gt $worst) { $worst = [double]$value }
    }
    return $worst
}

# The daemon's own infrastructure job also holds every enrolled lazooctl process that ran, so
# its CPU delta over a window is the keeper + daemon + CLI figure (no desktop). Informational.
function Get-JobWindow($Before, $After) {
    $cpuMillis = [long]$After.systemBudget.cpuMillis - [long]$Before.systemBudget.cpuMillis
    # ConvertFrom-Json turns ISO timestamps into DateTime; accept strings as well.
    $beforeAt = [DateTimeOffset]$Before.systemBudget.sampledAt
    $afterAt = [DateTimeOffset]$After.systemBudget.sampledAt
    $wallMillis = ($afterAt - $beforeAt).TotalMilliseconds
    return [ordered]@{
        cpuMillis = $cpuMillis
        wallMillis = [long]$wallMillis
        processesEnrolledInWindow = [long]$After.systemBudget.totalProcesses - [long]$Before.systemBudget.totalProcesses
        cpuShareIncludingCli = [double]$cpuMillis / ($wallMillis * $script:processorCount)
        daemonReportedRolling30sCpuPercentAtEnd = $After.systemBudget.thirtySeconds
    }
}

function Get-DaemonAccounting {
    $resources = Invoke-Json @('system', 'resources')
    $budget = Invoke-Json @('system', 'budget')
    return [ordered]@{
        sampledAt = [DateTime]::UtcNow.ToString('o')
        systemResources = $resources
        systemBudget = $budget
    }
}

$repoRoot = (Resolve-Path (Join-Path $PSScriptRoot '..\..')).Path
$releaseDir = Join-Path $repoRoot 'target\release'
$script:keeperExe = Join-Path $releaseDir 'lazoo-keeper.exe'
$script:daemonExe = Join-Path $releaseDir 'lazood.exe'
$script:ctlExe = Join-Path $releaseDir 'lazooctl.exe'
$script:desktopExe = Join-Path $releaseDir 'lazoo-desktop.exe'
$desktopDist = Join-Path $repoRoot 'apps\lazoo-desktop\ui\dist'
$artifactDir = Join-Path $repoRoot 'artifacts\acceptance'
$manifestPath = Join-Path $artifactDir 'desktop-budget-manifest.json'
$runId = [Guid]::NewGuid().ToString('N')
$runRoot = Join-Path ([IO.Path]::GetTempPath()) "lazoo-desktop-budget-$runId"
$script:dataRoot = Join-Path $runRoot 'data'
$projectRoot = Join-Path $runRoot 'repository'
$startedAt = [DateTime]::UtcNow
$stopwatch = [Diagnostics.Stopwatch]::StartNew()
$script:clock = [Diagnostics.Stopwatch]::StartNew()
$script:commands = [Collections.Generic.List[object]]::new()
$script:recordCommands = $true
$script:processorCount = [Environment]::ProcessorCount
$script:tracked = @{}
$script:everSeen = @{}
$script:unreadable = @{}
$script:hasSampled = $false
$script:lastSampleWall = [DateTime]::UtcNow
$script:lastSampleWallMs = 0.0
$script:keeperPid = 0
$script:daemonPid = 0
$script:desktopPid = 0
$script:keeperStdoutTask = $null
$script:keeperStderrTask = $null
$gates = [ordered]@{}
$measurements = [ordered]@{}
$measured = [ordered]@{}
$jobInclusive = $null
$daemonAccounting = [ordered]@{}
$keeper = $null
$desktop = $null
$manifestStatus = 'FAIL'
$failure = $null
$desktopSha256 = $null
$distIndexSha256 = $null
$distBuilt = $false
$exitCode = 1

# Section 20.1 targets, as shares of total machine capacity and bytes.
$idleCpuCeiling = 0.01
$routingCpuCeiling = 0.05
$idleMemoryCeilingBytes = 350MB
$activeMemoryCeilingBytes = 750MB
$seconds = if ($WindowSeconds -gt 0) { $WindowSeconds } elseif ($FullWindows) { 300 } else { 60 }

New-Item -ItemType Directory -Force -Path $artifactDir, $script:dataRoot, $projectRoot | Out-Null

try {
    if ($Build -and $SkipBuild) { throw '-Build and -SkipBuild contradict each other' }
    if ($Build) {
        $npmExe = (Get-Command npm.cmd -CommandType Application -ErrorAction Stop).Source
        $cargoExe = (Get-Command cargo -CommandType Application -ErrorAction Stop).Source
        Push-Location $repoRoot
        try {
            foreach ($step in @(
                @{ label = 'npm --prefix apps/lazoo-desktop/ui ci'; exe = $npmExe; args = @('--prefix', 'apps/lazoo-desktop/ui', 'ci') },
                @{ label = 'npm --prefix apps/lazoo-desktop/ui run build'; exe = $npmExe; args = @('--prefix', 'apps/lazoo-desktop/ui', 'run', 'build') },
                @{ label = 'cargo build -p lazoo-desktop --release --locked'; exe = $cargoExe; args = @('build', '-p', 'lazoo-desktop', '--release', '--locked') }
            )) {
                $timer = [Diagnostics.Stopwatch]::StartNew()
                & $step.exe @($step.args)
                $stepExit = $LASTEXITCODE
                $timer.Stop()
                $script:commands.Add([ordered]@{ command = $step.label; exitCode = $stepExit; durationMs = $timer.ElapsedMilliseconds })
                Assert-Equal $stepExit 0 $step.label
            }
        } finally {
            Pop-Location
        }
        $distBuilt = $true
    }
    $gates.build = if ($Build) { 'PASS' } else { 'SKIPPED_BY_CALLER' }

    foreach ($binary in @($script:keeperExe, $script:daemonExe, $script:ctlExe, $script:desktopExe)) {
        Assert-True (Test-Path -LiteralPath $binary) "binary exists: $binary"
    }
    Assert-True (Test-Path -LiteralPath (Join-Path $desktopDist 'index.html')) 'packaged UI assets exist (run with -Build to create them)'
    $desktopSha256 = (Get-FileHash -LiteralPath $script:desktopExe -Algorithm SHA256).Hash.ToLowerInvariant()
    $distIndexSha256 = (Get-FileHash -LiteralPath (Join-Path $desktopDist 'index.html') -Algorithm SHA256).Hash.ToLowerInvariant()

    Add-Type -AssemblyName UIAutomationClient

    & git init --quiet $projectRoot
    Assert-Equal $LASTEXITCODE 0 'temporary Git repository initialization'

    $keeper = Start-Keeper $script:dataRoot
    $script:keeperPid = $keeper.Id
    $status = Wait-Status
    Assert-Equal $status.schemaVersion 19 'schema version'
    $gates.schemaVersion = 'PASS'
    $registration = Invoke-Json @('project', 'register', $projectRoot, '--name', 'Desktop budget', '--idempotency-key', "desktop-budget-$runId")
    Assert-True ($null -ne $registration.project.id) 'project registered before the desktop starts'

    $startResources = Invoke-Json @('system', 'resources')
    $script:daemonPid = [int]$startResources.pid
    $daemonProcess = Get-Process -Id $script:daemonPid
    Assert-Equal $daemonProcess.Path $script:daemonExe 'system.resources names the real daemon'

    $desktop = Start-Desktop $script:dataRoot
    $script:desktopPid = $desktop.Id
    $windowHandle = Wait-Window $desktop
    $desktop.Refresh()
    $expectedTitle = 'Lazoo ' + [char]0x2014 + ' Local control room'
    Assert-Equal $desktop.MainWindowTitle $expectedTitle 'desktop window title'
    # Same readiness evidence as desktop-shell.ps1: the UI reports a steady control plane and
    # shows the registered project, so it has fetched a snapshot from the connected daemon.
    $null = Wait-UiText $windowHandle @('Control plane steady', 'Open command palette')
    $gates.desktopConnectedToDaemon = 'PASS'

    # Let WebView2 start-up and first render finish before the first window.
    $settleDeadline = [DateTime]::UtcNow.AddSeconds($SettleSeconds)
    $script:recordCommands = $false
    Update-Roster
    while ([DateTime]::UtcNow -lt $settleDeadline) {
        Update-Roster
        Start-Sleep -Seconds 2
    }
    Update-Roster
    $webviewCount = @($script:tracked.Values | Where-Object { $_.group -eq 'webview2' }).Count
    Assert-True ($webviewCount -ge 1) 'the desktop has WebView2 descendant processes'
    $gates.webView2Attributed = 'PASS'

    $daemonAccounting.beforeIdle = Get-DaemonAccounting

    # (1) Idle: desktop open, daemon idle, nothing driving either.
    $idle = New-Window 'idle' $seconds
    Invoke-Window $idle
    $daemonAccounting.afterIdle = Get-DaemonAccounting
    $measurements.idle = Get-WindowSummary $idle

    # (2) Routing: the same deterministic control-plane activity as resource-budget.ps1 (reads
    # plus one durable write, fake transport, no model), desktop still open and refreshing.
    $routing = New-Window 'routing' $seconds
    $routingCalls = 0
    $workload = {
        Invoke-Json @('status') | Out-Null
        Invoke-Json @('project', 'list') | Out-Null
        Invoke-Json @('events', 'list', '--after', '0', '--limit', '100') | Out-Null
        Invoke-Json @('inbox', 'counts') | Out-Null
        Invoke-Json @('roster', 'show') | Out-Null
        $script:routingCalls += 5
    }
    $script:routingCalls = 0
    $secondProject = Join-Path $runRoot 'repository-routing'
    New-Item -ItemType Directory -Force -Path $secondProject | Out-Null
    & git init --quiet $secondProject
    $routingRegistration = Invoke-Json @('project', 'register', $secondProject, '--idempotency-key', "desktop-budget-routing-$runId")
    Assert-True ($null -ne $routingRegistration.project.id) 'project registered during the routing window'
    Invoke-Window $routing $workload
    $routing.extra.calls = $script:routingCalls
    $routing.extra.workload = 'status, project list, events list, inbox counts, roster show in a loop, plus one project registration'
    $daemonAccounting.afterRouting = Get-DaemonAccounting
    $measurements.routing = Get-WindowSummary $routing
    Assert-True ($script:routingCalls -ge 5) 'routing window issued calls'

    $script:recordCommands = $true
    $resources = Invoke-Json @('system', 'resources')
    Assert-Equal ([int]$resources.liveSessions) 0 'no provider session was started'
    $gates.noProviderProcess = 'PASS'

    # Verdicts per target. CPU uses the strictest rolling reading the window can produce: the
    # worst rolling 30-second mean, and when the window is long enough the worst 5-minute mean.
    $idleSummary = $measurements.idle
    $routingSummary = $measurements.routing
    $idleCpuWorst = Get-WorstShare $idleSummary
    $routingCpuWorst = Get-WorstShare $routingSummary
    $gates.idleCpuUnderOnePercent = if ($idleCpuWorst -lt $idleCpuCeiling) { 'PASS' } else { 'FAIL' }
    $gates.idleMemoryUnder350MiB = if ($idleSummary.workingSetBytesPeak -lt $idleMemoryCeilingBytes) { 'PASS' } else { 'FAIL' }
    $gates.routingCpuUnderFivePercent = if ($routingCpuWorst -lt $routingCpuCeiling) { 'PASS' } else { 'FAIL' }
    $gates.activeMemoryUnder750MiB = if ($routingSummary.workingSetBytesPeak -lt $activeMemoryCeilingBytes) { 'PASS' } else { 'FAIL' }
    $jobInclusive = [ordered]@{
        idle = Get-JobWindow $daemonAccounting.beforeIdle $daemonAccounting.afterIdle
        routing = Get-JobWindow $daemonAccounting.afterIdle $daemonAccounting.afterRouting
        note = 'informational, not gated: the daemon infrastructure job (keeper, daemon and every lazooctl that ran; desktop excluded). The routing figure includes the lazooctl load generator.'
    }
    $measured.idle = [ordered]@{
        cpuShareGated = $idleCpuWorst
        cpuShareTarget = $idleCpuCeiling
        workingSetBytesGated = $idleSummary.workingSetBytesPeak
        workingSetBytesTarget = [long]$idleMemoryCeilingBytes
    }
    $measured.routing = [ordered]@{
        cpuShareGated = $routingCpuWorst
        cpuShareTarget = $routingCpuCeiling
        workingSetBytesGated = $routingSummary.workingSetBytesPeak
        workingSetBytesTarget = [long]$activeMemoryCeilingBytes
        workingSetBytesAlsoUnderIdleTarget = ($routingSummary.workingSetBytesPeak -lt $idleMemoryCeilingBytes)
    }
    if ($script:unreadable.Count -gt 0) {
        $gates.allProcessesReadable = "FAIL: $($script:unreadable.Keys -join ', ')"
    } else {
        $gates.allProcessesReadable = 'PASS'
    }

    # Close the desktop first (as desktop-shell.ps1 does), then the planned daemon shutdown.
    if (-not $desktop.CloseMainWindow()) { Stop-Process -Id $desktop.Id -Force -ErrorAction SilentlyContinue }
    if (-not $desktop.WaitForExit(8000)) { Stop-Tree $desktop.Id }
    Assert-True (-not $keeper.HasExited) 'closing the desktop leaves the keeper running'
    Invoke-Json @('system', 'shutdown') | Out-Null
    Assert-True $keeper.WaitForExit(15000) 'keeper exits after planned daemon shutdown'
    Assert-Equal $keeper.ExitCode 0 'planned keeper exit code'
    $gates.plannedShutdown = 'PASS'
} catch {
    $failure = $_.Exception.Message
    $gates.failure = $failure
} finally {
    try {
        if ($null -ne $desktop -and -not $desktop.HasExited) { Stop-Tree $desktop.Id }
        if ($null -ne $keeper -and -not $keeper.HasExited) {
            Stop-Tree $keeper.Id
        }
    } catch {
        $gates.cleanup = "FAIL: $($_.Exception.Message)"
    }
    # Everything this run ever tracked must be gone.
    $leftover = @($script:everSeen.Keys | Where-Object { $null -ne (Get-Process -Id $_ -ErrorAction SilentlyContinue) })
    $leftover = @($leftover | Where-Object { $name = $script:everSeen[$_]; $proc = Get-Process -Id $_ -ErrorAction SilentlyContinue; $null -ne $proc -and $proc.ProcessName -eq $name })
    foreach ($id in $leftover) { Stop-Process -Id $id -Force -ErrorAction SilentlyContinue }
    Start-Sleep -Milliseconds 300
    $stillThere = @($leftover | Where-Object { $null -ne (Get-Process -Id $_ -ErrorAction SilentlyContinue) })
    $gates.cleanup = if ($stillThere.Count -eq 0) { 'PASS' } else { "FAIL: processes still running: $($stillThere -join ', ')" }
    foreach ($item in $script:tracked.Values) { try { $item.process.Dispose() } catch { } }

    $keeperStdout = ''
    $keeperStderr = ''
    try { if ($null -ne $script:keeperStdoutTask -and $script:keeperStdoutTask.Wait(2000)) { $keeperStdout = $script:keeperStdoutTask.Result } } catch { }
    try { if ($null -ne $script:keeperStderrTask -and $script:keeperStderrTask.Wait(2000)) { $keeperStderr = $script:keeperStderrTask.Result } } catch { }

    $allPass = $null -eq $failure
    foreach ($key in $gates.Keys) {
        if ($key -ne 'failure' -and [string]$gates[$key] -like 'FAIL*') { $allPass = $false }
    }
    $requiredGates = @('schemaVersion', 'desktopConnectedToDaemon', 'webView2Attributed', 'idleCpuUnderOnePercent', 'idleMemoryUnder350MiB', 'routingCpuUnderFivePercent', 'activeMemoryUnder750MiB', 'noProviderProcess', 'plannedShutdown', 'allProcessesReadable', 'cleanup')
    foreach ($key in $requiredGates) {
        if (-not $gates.Contains($key) -or [string]$gates[$key] -ne 'PASS') { $allPass = $false }
    }
    $manifestStatus = if ($allPass) { 'PASS' } else { 'FAIL' }
    $stopwatch.Stop()

    $processList = @()
    foreach ($processId in ($script:everSeen.Keys | Sort-Object { [int]$_ })) {
        $item = $script:tracked[[int]$processId]
        $processList += [ordered]@{
            pid = [int]$processId
            name = [string]$script:everSeen[$processId]
            group = if ($null -ne $item) { $item.group } else { $null }
        }
    }
    $manifest = [ordered]@{
        status = $manifestStatus
        schemaVersion = 19
        mode = 'real-binaries-with-desktop'
        startedAt = $startedAt.ToString('o')
        durationMs = [long]$stopwatch.ElapsedMilliseconds
        processorCount = $script:processorCount
        failure = $failure
        targets = [ordered]@{
            idleCpuShare = $idleCpuCeiling
            routingCpuShare = $routingCpuCeiling
            idleWorkingSetBytes = [long]$idleMemoryCeilingBytes
            activeWorkingSetBytes = [long]$activeMemoryCeilingBytes
            windowSeconds = $seconds
            fullWindows = [bool]$FullWindows
            settleSeconds = $SettleSeconds
            sampleIntervalMillis = 1000
            cpuGate = 'worst rolling 30-second mean (and worst rolling 5-minute mean when the window is at least 300 seconds), as a share of total machine capacity'
            memoryGate = 'peak summed working set across the measured processes'
        }
        verdicts = $measured
        measurements = $measurements
        infrastructureJobIncludingCli = $jobInclusive
        daemonAccounting = $daemonAccounting
        processes = $processList
        gates = $gates
        commands = @($script:commands)
        binaries = [ordered]@{
            desktop = $script:desktopExe
            desktopSha256 = $desktopSha256
            desktopUiDistIndexSha256 = $distIndexSha256
            uiAssetsBuiltByThisRun = $distBuilt
        }
        keeper = [ordered]@{ stdoutBytes = $keeperStdout.Length; stderrBytes = $keeperStderr.Length; stderr = $keeperStderr }
        notes = [ordered]@{
            memory = 'WorkingSet64 summed across processes counts pages shared between WebView2 processes more than once, so it is an upper bound on resident memory; PrivateMemorySize64 peak is recorded beside it.'
            readiness = 'desktop readiness used one UI Automation probe (as desktop-shell.ps1 does) before the settle period; that probe can switch WebView2 accessibility on, which makes the figures conservative.'
            cli = 'the gated figures do not count the lazooctl processes that drive the routing window (the load generator), as in resource-budget.ps1. The daemon infrastructure job does enroll them, so infrastructureJobIncludingCli records the keeper + daemon + CLI figure beside it.'
            webview2 = 'WebView2 processes are attributed as descendants of the desktop process by parent PID; WebView2 processes of other applications are not counted.'
        }
        boundaries = [ordered]@{
            mode = 'real-binaries-with-desktop'
            desktopUi = 'UNCHANGED'
            modelUse = 'NONE'
            providerSmoke = 'NOT_RUN'
            externalModelUse = 'NOT_RUN'
            measured = 'KEEPER_DAEMON_DESKTOP_AND_WEBVIEW2_DESCENDANTS'
            notMeasured = 'CLI_LOAD_GENERATOR_FAILURE_STORM_MEMORY_INDEXING_TERMINAL_STREAMING_VOICE'
            scenarios = 'IDLE_AND_ROUTING_WITH_DESKTOP_OPEN'
            enforcement = 'NONE'
        }
    }
    $manifest | ConvertTo-Json -Depth 9 | Set-Content -LiteralPath $manifestPath -Encoding UTF8
}

if ($manifestStatus -eq 'PASS') {
    Write-Output "PASS manifest: $manifestPath"
    exit 0
}
Write-Output "FAIL manifest: $manifestPath"
if ($null -ne $failure) { Write-Output "failure: $failure" }
foreach ($key in $gates.Keys) { Write-Output "  ${key}: $($gates[$key])" }
exit 1
