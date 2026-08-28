param(
    [switch]$SkipBuild
)

$ErrorActionPreference = 'Stop'
Set-StrictMode -Version Latest

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
        $timer = [Diagnostics.Stopwatch]::StartNew()
        $captured = @(& $script:ctlExe --json @Arguments 2>&1)
        $exitCode = $LASTEXITCODE
        $timer.Stop()
        $script:commands.Add([ordered]@{
            command = "lazooctl --json $($Arguments -join ' ')"
            exitCode = $exitCode
            durationMs = $timer.ElapsedMilliseconds
        })
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
        try { return Invoke-Json @('status') } catch { $lastError = $_.Exception.Message }
        Start-Sleep -Milliseconds 100
    }
    throw "daemon did not become ready: $lastError"
}

function Start-HiddenProcess([string]$Executable, [string]$DataRoot, [bool]$RedirectOutput) {
    $startInfo = [Diagnostics.ProcessStartInfo]::new()
    $startInfo.FileName = $Executable
    $startInfo.UseShellExecute = $false
    $startInfo.CreateNoWindow = $RedirectOutput
    $startInfo.RedirectStandardOutput = $RedirectOutput
    $startInfo.RedirectStandardError = $RedirectOutput
    $startInfo.EnvironmentVariables['LAZOO_DATA_ROOT'] = $DataRoot
    return [Diagnostics.Process]::Start($startInfo)
}

function Get-DescendantProcessIds([int]$RootProcessId) {
    $processes = @(Get-CimInstance Win32_Process | Select-Object ProcessId, ParentProcessId)
    $pending = [Collections.Generic.Queue[int]]::new()
    $seen = [Collections.Generic.HashSet[int]]::new()
    $descendants = [Collections.Generic.List[int]]::new()
    $pending.Enqueue($RootProcessId)
    $seen.Add($RootProcessId) | Out-Null

    while ($pending.Count -gt 0) {
        $parentId = $pending.Dequeue()
        foreach ($candidate in @($processes | Where-Object { $_.ParentProcessId -eq $parentId })) {
            $candidateId = [int]$candidate.ProcessId
            if ($seen.Add($candidateId)) {
                $descendants.Add($candidateId)
                $pending.Enqueue($candidateId)
            }
        }
    }

    return @($descendants)
}

function Wait-DescendantProcess(
    [int]$RootProcessId,
    [string]$ProcessName,
    [int]$TimeoutSeconds = 10
) {
    $deadline = [DateTime]::UtcNow.AddSeconds($TimeoutSeconds)
    while ([DateTime]::UtcNow -lt $deadline) {
        foreach ($candidateId in @(Get-DescendantProcessIds $RootProcessId)) {
            $candidate = Get-Process -Id $candidateId -ErrorAction SilentlyContinue
            if ($null -ne $candidate -and "$($candidate.ProcessName).exe" -eq $ProcessName) {
                return $candidate
            }
        }
        Start-Sleep -Milliseconds 100
    }
    throw "process $RootProcessId did not start a descendant named $ProcessName"
}

function Stop-OwnedProcessTree([int]$RootProcessId) {
    $descendantIds = @(Get-DescendantProcessIds $RootProcessId)

    Stop-Process -Id $RootProcessId -Force -ErrorAction SilentlyContinue
    [array]::Reverse($descendantIds)
    foreach ($candidateId in $descendantIds) {
        Stop-Process -Id $candidateId -Force -ErrorAction SilentlyContinue
    }
}

function Wait-Window([Diagnostics.Process]$Process, [int]$TimeoutSeconds = 20) {
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

function Wait-UiText([IntPtr]$WindowHandle, [string[]]$Expected, [int]$TimeoutSeconds = 20) {
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

function Capture-Window([IntPtr]$WindowHandle, [string]$Path) {
    Assert-True ([WindowNative]::BringToFront($WindowHandle)) 'window brought to foreground'
    Start-Sleep -Milliseconds 350
    $bounds = [WindowNative]::GetBounds($WindowHandle)
    Assert-True ($bounds.Width -gt 0 -and $bounds.Height -gt 0) 'window screenshot bounds'
    $bitmap = [Drawing.Bitmap]::new($bounds.Width, $bounds.Height)
    $graphics = [Drawing.Graphics]::FromImage($bitmap)
    try {
        $deviceContext = $graphics.GetHdc()
        try {
            $captured = [WindowNative]::PrintCompleteWindow($WindowHandle, $deviceContext)
        } finally {
            $graphics.ReleaseHdc($deviceContext)
        }
        if (-not $captured) {
            $graphics.CopyFromScreen($bounds.Left, $bounds.Top, 0, 0, $bitmap.Size)
        }
        $bitmap.Save($Path, [Drawing.Imaging.ImageFormat]::Png)
    } finally {
        $graphics.Dispose()
        $bitmap.Dispose()
    }
    Assert-True ((Get-Item -LiteralPath $Path).Length -gt 10KB) "screenshot content: $Path"
    return $bounds
}

Add-Type -AssemblyName System.Drawing
Add-Type -AssemblyName UIAutomationClient
Add-Type -TypeDefinition @'
using System;
using System.Runtime.InteropServices;

public static class WindowNative {
    [StructLayout(LayoutKind.Sequential)]
    private struct RECT { public int Left; public int Top; public int Right; public int Bottom; }

    public sealed class Bounds {
        public int Left { get; set; }
        public int Top { get; set; }
        public int Width { get; set; }
        public int Height { get; set; }
    }

    [DllImport("user32.dll", SetLastError = true)]
    private static extern bool GetWindowRect(IntPtr window, out RECT rect);

    [DllImport("user32.dll", SetLastError = true)]
    public static extern bool MoveWindow(IntPtr window, int x, int y, int width, int height, bool repaint);

    [DllImport("user32.dll")]
    private static extern bool ShowWindow(IntPtr window, int command);

    [DllImport("user32.dll")]
    private static extern bool SetForegroundWindow(IntPtr window);

    [DllImport("user32.dll", SetLastError = true)]
    private static extern bool SetWindowPos(
        IntPtr window,
        IntPtr insertAfter,
        int x,
        int y,
        int width,
        int height,
        uint flags
    );

    [DllImport("user32.dll", SetLastError = true)]
    private static extern bool PrintWindow(IntPtr window, IntPtr deviceContext, uint flags);

    public static bool BringToFront(IntPtr window) {
        const uint NoMove = 0x0002;
        const uint NoSize = 0x0001;
        const uint Show = 0x0040;
        ShowWindow(window, 9);
        bool raised = SetWindowPos(window, new IntPtr(-1), 0, 0, 0, 0, NoMove | NoSize | Show);
        SetForegroundWindow(window);
        return raised;
    }

    public static bool PrintCompleteWindow(IntPtr window, IntPtr deviceContext) {
        return PrintWindow(window, deviceContext, 2);
    }

    public static Bounds GetBounds(IntPtr window) {
        RECT rect;
        if (!GetWindowRect(window, out rect)) {
            throw new System.ComponentModel.Win32Exception(Marshal.GetLastWin32Error());
        }
        return new Bounds {
            Left = rect.Left,
            Top = rect.Top,
            Width = rect.Right - rect.Left,
            Height = rect.Bottom - rect.Top
        };
    }
}
'@

$repoRoot = (Resolve-Path (Join-Path $PSScriptRoot '..\..')).Path
$cargoExe = (Get-Command cargo -CommandType Application -ErrorAction Stop).Source
$releaseDir = Join-Path $repoRoot 'target\release'
$script:keeperExe = Join-Path $releaseDir 'lazoo-keeper.exe'
$script:daemonExe = Join-Path $releaseDir 'lazood.exe'
$script:ctlExe = Join-Path $releaseDir 'lazooctl.exe'
$desktopExe = Join-Path $releaseDir 'lazoo-desktop.exe'
$artifactDir = Join-Path $repoRoot 'artifacts\acceptance'
$manifestPath = Join-Path $artifactDir 'desktop-shell-manifest.json'
$runId = [Guid]::NewGuid().ToString('N')
$runRoot = Join-Path ([IO.Path]::GetTempPath()) "lazoo-desktop-acceptance-$runId"
$script:dataRoot = Join-Path $runRoot 'data'
$projectRoot = Join-Path $runRoot 'repository'
$cleanupOwnedRoot = Join-Path $runRoot 'cleanup-owned'
$cleanupUnrelatedRoot = Join-Path $runRoot 'cleanup-unrelated'
$defaultScreenshot = Join-Path $runRoot 'desktop-default.png'
$narrowScreenshot = Join-Path $runRoot 'desktop-narrow.png'
$startedAt = [DateTime]::UtcNow
$stopwatch = [Diagnostics.Stopwatch]::StartNew()
$script:commands = [Collections.Generic.List[object]]::new()
$gates = [ordered]@{}
$keeper = $null
$desktop = $null
$cleanupProbeKeeper = $null
$cleanupProbeOther = $null
$manifestStatus = 'FAIL'
$keeperStdout = ''
$keeperStderr = ''

New-Item -ItemType Directory -Force -Path @(
    $artifactDir,
    $script:dataRoot,
    $projectRoot,
    $cleanupOwnedRoot,
    $cleanupUnrelatedRoot
) | Out-Null

try {
    if (-not $SkipBuild) {
        $timer = [Diagnostics.Stopwatch]::StartNew()
        & $cargoExe build --workspace --release --locked
        $buildExit = $LASTEXITCODE
        $timer.Stop()
        $script:commands.Add([ordered]@{
            command = 'cargo build --workspace --release --locked'
            exitCode = $buildExit
            durationMs = $timer.ElapsedMilliseconds
        })
        Assert-Equal $buildExit 0 'locked release build'
    }
    foreach ($binary in @($script:keeperExe, $script:daemonExe, $script:ctlExe, $desktopExe)) {
        Assert-True (Test-Path -LiteralPath $binary) "binary exists: $binary"
    }
    $gates.releaseBuild = 'PASS'

    $cleanupProbeKeeper = Start-HiddenProcess $script:keeperExe $cleanupOwnedRoot $true
    $ownedProbeDaemon = Wait-DescendantProcess $cleanupProbeKeeper.Id 'lazood.exe'
    $cleanupProbeOther = Start-HiddenProcess $script:daemonExe $cleanupUnrelatedRoot $true
    Start-Sleep -Milliseconds 500
    Assert-True (-not $cleanupProbeOther.HasExited) 'unrelated cleanup-probe daemon started'

    Stop-OwnedProcessTree $cleanupProbeKeeper.Id
    Assert-True $cleanupProbeKeeper.WaitForExit(5000) 'cleanup-probe keeper stopped'
    Assert-True (
        $null -eq (Get-Process -Id $ownedProbeDaemon.Id -ErrorAction SilentlyContinue)
    ) 'cleanup-probe owned daemon stopped'
    Assert-True (-not $cleanupProbeOther.HasExited) 'cleanup preserves unrelated daemon from same binary'
    $cleanupProbeKeeper = $null
    Stop-Process -Id $cleanupProbeOther.Id -Force
    Assert-True $cleanupProbeOther.WaitForExit(5000) 'unrelated cleanup-probe daemon stopped explicitly'
    $cleanupProbeOther = $null
    $gates.cleanupIsolation = 'PASS'

    & git init --quiet $projectRoot
    Assert-Equal $LASTEXITCODE 0 'temporary Git repository initialization'
    $script:commands.Add([ordered]@{
        command = 'git init --quiet <temporary-project>'
        exitCode = $LASTEXITCODE
        durationMs = $null
    })

    $keeper = Start-HiddenProcess $script:keeperExe $script:dataRoot $true
    $status = Wait-Status
    Assert-Equal $status.protocolVersion 1 'protocol version'
    Assert-Equal $status.journalMode 'wal' 'SQLite journal mode'
    $gates.daemonReady = 'PASS'

    $registration = Invoke-Json @(
        'project', 'register', $projectRoot,
        '--name', 'Desktop acceptance',
        '--idempotency-key', "desktop-$runId"
    )
    Assert-Equal $registration.project.displayName 'Desktop acceptance' 'desktop project name'

    $desktop = Start-HiddenProcess $desktopExe $script:dataRoot $false
    $windowHandle = Wait-Window $desktop
    $desktop.Refresh()
    $expectedTitle = 'Lazoo ' + [char]0x2014 + ' Local control room'
    Assert-Equal $desktop.MainWindowTitle $expectedTitle 'desktop window title'
    $gates.desktopWindow = 'PASS'

    $uiNames = Wait-UiText $windowHandle @('Control plane steady', 'Desktop acceptance')
    Assert-True ('Project registered' -in $uiNames) 'desktop recent event label'
    $gates.snapshotRpc = 'PASS'

    $defaultBounds = Capture-Window $windowHandle $defaultScreenshot
    Assert-True ($defaultBounds.Width -ge 1100 -and $defaultBounds.Height -ge 700) 'default window dimensions'
    $gates.defaultScreenshot = 'PASS'

    Assert-True ([WindowNative]::MoveWindow($windowHandle, 80, 80, 860, 620, $true)) 'narrow window resize'
    Start-Sleep -Milliseconds 750
    $narrowBounds = Capture-Window $windowHandle $narrowScreenshot
    Assert-Equal $narrowBounds.Width 860 'narrow window width'
    Assert-Equal $narrowBounds.Height 620 'narrow window height'
    Wait-UiText $windowHandle @('System pulse', 'Desktop acceptance') | Out-Null
    $gates.narrowScreenshot = 'PASS'

    Assert-True $desktop.CloseMainWindow() 'desktop accepted close request'
    if (-not $desktop.WaitForExit(5000)) { Stop-Process -Id $desktop.Id -Force }
    $desktop = $null
    Assert-True (-not $keeper.HasExited) 'closing desktop leaves keeper running'
    $gates.processIsolation = 'PASS'

    Invoke-Json @('system', 'shutdown') | Out-Null
    Assert-True $keeper.WaitForExit(15000) 'keeper exits after planned daemon shutdown'
    $keeperStdout = $keeper.StandardOutput.ReadToEnd()
    $keeperStderr = $keeper.StandardError.ReadToEnd()
    Assert-Equal $keeper.ExitCode 0 'planned keeper exit code'
    $keeper = $null
    $manifestStatus = 'PASS'
} catch {
    $gates.failure = $_.Exception.Message
    throw
} finally {
    $stopwatch.Stop()
    if ($null -ne $desktop -and -not $desktop.HasExited) {
        Stop-Process -Id $desktop.Id -Force
    }
    if ($null -ne $cleanupProbeOther -and -not $cleanupProbeOther.HasExited) {
        Stop-Process -Id $cleanupProbeOther.Id -Force
    }
    if ($null -ne $cleanupProbeKeeper -and -not $cleanupProbeKeeper.HasExited) {
        Stop-OwnedProcessTree $cleanupProbeKeeper.Id
    }
    if ($null -ne $keeper -and -not $keeper.HasExited) {
        Stop-OwnedProcessTree $keeper.Id
    }
    $manifest = [ordered]@{
        schemaVersion = 1
        status = $manifestStatus
        startedAt = $startedAt.ToString('o')
        completedAt = [DateTime]::UtcNow.ToString('o')
        durationMs = $stopwatch.ElapsedMilliseconds
        platform = [Environment]::OSVersion.VersionString
        rustcVersion = (& rustc --version)
        cargoVersion = (& $cargoExe --version)
        repositoryRoot = $repoRoot
        isolatedDataRoot = $script:dataRoot
        temporaryProject = $projectRoot
        artifacts = [ordered]@{
            manifest = $manifestPath
            runRoot = $runRoot
            defaultScreenshot = $defaultScreenshot
            narrowScreenshot = $narrowScreenshot
        }
        keeper = [ordered]@{
            stdout = $keeperStdout
            stderr = $keeperStderr
        }
        commands = @($script:commands)
        gates = $gates
    }
    $manifest | ConvertTo-Json -Depth 8 | Set-Content -LiteralPath $manifestPath -Encoding UTF8
}

Write-Output "DESKTOP SHELL ACCEPTANCE: PASS"
Write-Output "PASS manifest: $manifestPath"
Write-Output "Visual QA screenshots: $defaultScreenshot and $narrowScreenshot"
