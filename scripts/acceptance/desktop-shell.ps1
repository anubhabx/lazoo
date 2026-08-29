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

function Start-HiddenProcess(
    [string]$Executable,
    [string]$DataRoot,
    [bool]$RedirectOutput,
    [string]$Arguments = ''
) {
    $startInfo = [Diagnostics.ProcessStartInfo]::new()
    $startInfo.FileName = $Executable
    $startInfo.Arguments = $Arguments
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

function Stop-OwnedProcess(
    [Diagnostics.Process]$Process,
    [int]$TimeoutMilliseconds = 5000,
    [int]$ProbeDelayMilliseconds = 0
) {
    if ($null -eq $Process) { return $true }
    try {
        if ($ProbeDelayMilliseconds -gt 0) {
            Start-Sleep -Milliseconds $ProbeDelayMilliseconds
        }
        Stop-Process -Id $Process.Id -Force -ErrorAction SilentlyContinue
        return $Process.WaitForExit($TimeoutMilliseconds)
    } catch {
        try { return $Process.HasExited } catch { return $false }
    }
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

function Stop-OwnedProcessTree(
    [int]$RootProcessId,
    [int]$PreStopProbeDelayMilliseconds = 0,
    [int]$RootStopProbeDelayMilliseconds = 0
) {
    $observedDescendants = [Collections.Generic.HashSet[int]]::new()
    foreach ($candidateId in @(Get-DescendantProcessIds $RootProcessId)) {
        $observedDescendants.Add($candidateId) | Out-Null
    }

    if ($PreStopProbeDelayMilliseconds -gt 0) {
        Start-Sleep -Milliseconds $PreStopProbeDelayMilliseconds
    }
    foreach ($candidateId in @(Get-DescendantProcessIds $RootProcessId)) {
        $observedDescendants.Add($candidateId) | Out-Null
    }

    $rootProcess = Get-Process -Id $RootProcessId -ErrorAction SilentlyContinue
    if ($null -ne $rootProcess) {
        Assert-True (
            Stop-OwnedProcess $rootProcess 5000 $RootStopProbeDelayMilliseconds
        ) "process-tree root $RootProcessId stopped"
    }

    $deadline = [DateTime]::UtcNow.AddSeconds(5)
    do {
        $remainingIds = @(Get-DescendantProcessIds $RootProcessId)
        foreach ($candidateId in $remainingIds) {
            $observedDescendants.Add($candidateId) | Out-Null
            Stop-Process -Id $candidateId -Force -ErrorAction SilentlyContinue
        }
        if ($remainingIds.Count -eq 0) { break }
        Start-Sleep -Milliseconds 50
    } while ([DateTime]::UtcNow -lt $deadline)

    $remainingIds = @(Get-DescendantProcessIds $RootProcessId)
    Assert-Equal $remainingIds.Count 0 "process-tree descendants for $RootProcessId"
    return @($observedDescendants)
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

function Wait-UiTextAbsent([IntPtr]$WindowHandle, [string[]]$Unexpected, [int]$TimeoutSeconds = 10) {
    $deadline = [DateTime]::UtcNow.AddSeconds($TimeoutSeconds)
    $lastNames = @()
    while ([DateTime]::UtcNow -lt $deadline) {
        try {
            $lastNames = @(Get-AutomationNames $WindowHandle)
            $present = @($Unexpected | Where-Object { $_ -in $lastNames })
            if ($present.Count -eq 0) { return }
        } catch { }
        Start-Sleep -Milliseconds 100
    }
    throw "desktop UI still exposed unexpected text '$($Unexpected -join ', ')'; observed: $($lastNames -join ' | ')"
}

function Wait-AutomationElement(
    [IntPtr]$WindowHandle,
    [string]$Name,
    [bool]$StartsWith = $false,
    [int]$TimeoutSeconds = 20
) {
    $deadline = [DateTime]::UtcNow.AddSeconds($TimeoutSeconds)
    while ([DateTime]::UtcNow -lt $deadline) {
        try {
            $root = [System.Windows.Automation.AutomationElement]::FromHandle($WindowHandle)
            $elements = $root.FindAll(
                [System.Windows.Automation.TreeScope]::Descendants,
                [System.Windows.Automation.Condition]::TrueCondition
            )
            foreach ($item in $elements) {
                try {
                    $candidateName = $item.Current.Name
                    $matches = if ($StartsWith) {
                        $candidateName.StartsWith($Name, [StringComparison]::Ordinal)
                    } else {
                        $candidateName -eq $Name
                    }
                    if ($matches) { return $item }
                } catch { }
            }
        } catch { }
        Start-Sleep -Milliseconds 100
    }
    throw "desktop UI did not expose automation element '$Name'"
}

function Send-WindowKeys(
    [IntPtr]$WindowHandle,
    [string]$Keys,
    [System.Windows.Automation.AutomationElement]$FocusElement = $null
) {
    Assert-True ([WindowNative]::BringToFront($WindowHandle)) 'window activated for keyboard input'
    Start-Sleep -Milliseconds 150
    if ($null -ne $FocusElement) {
        $FocusElement.SetFocus()
        Start-Sleep -Milliseconds 100
    }
    [System.Windows.Forms.SendKeys]::SendWait($Keys)
    Start-Sleep -Milliseconds 150
}

function Invoke-AutomationElement([System.Windows.Automation.AutomationElement]$Element) {
    $pattern = $null
    if (-not $Element.TryGetCurrentPattern(
        [System.Windows.Automation.InvokePattern]::Pattern,
        [ref]$pattern
    )) {
        throw "automation element '$($Element.Current.Name)' does not expose InvokePattern"
    }
    ([System.Windows.Automation.InvokePattern]$pattern).Invoke()
}

function Assert-FocusedAutomationName([string]$Prefix, [string]$Label) {
    $deadline = [DateTime]::UtcNow.AddSeconds(5)
    $lastName = ''
    while ([DateTime]::UtcNow -lt $deadline) {
        try {
            $focused = [System.Windows.Automation.AutomationElement]::FocusedElement
            $lastName = $focused.Current.Name
            if ($lastName.StartsWith($Prefix, [StringComparison]::Ordinal)) { return }
        } catch { }
        Start-Sleep -Milliseconds 100
    }
    throw "$Label expected focused name starting '$Prefix' but found '$lastName'"
}

function Test-DesktopRuntimeAssets([string]$DistRoot) {
    $indexPath = Join-Path $DistRoot 'index.html'
    Assert-True (Test-Path -LiteralPath $indexPath) 'desktop dist index exists'
    $index = Get-Content -LiteralPath $indexPath -Raw

    $inlineScripts = [regex]::Matches(
        $index,
        '<script\b(?![^>]*\bsrc\s*=)[^>]*>[\s\S]*?</script>',
        [Text.RegularExpressions.RegexOptions]::IgnoreCase
    )
    Assert-Equal $inlineScripts.Count 0 'inline script content'
    Assert-True (-not [regex]::IsMatch(
        $index,
        '\son[a-z]+\s*=',
        [Text.RegularExpressions.RegexOptions]::IgnoreCase
    )) 'inline event handler content'

    $references = [Collections.Generic.List[string]]::new()
    $attributeMatches = [regex]::Matches(
        $index,
        '(?:src|href)\s*=\s*["''](?<path>[^"'']+)["'']',
        [Text.RegularExpressions.RegexOptions]::IgnoreCase
    )
    foreach ($match in $attributeMatches) {
        $reference = $match.Groups['path'].Value
        Assert-True (-not ($reference -match '^(?:https?:|//|data:|javascript:)')) "remote runtime asset: $reference"
        Assert-True $reference.StartsWith('./', [StringComparison]::Ordinal) "local runtime asset path: $reference"
        $relativePath = $reference.Substring(2).Replace('/', [IO.Path]::DirectorySeparatorChar)
        $assetPath = [IO.Path]::GetFullPath((Join-Path $DistRoot $relativePath))
        Assert-True $assetPath.StartsWith(
            ([IO.Path]::GetFullPath($DistRoot) + [IO.Path]::DirectorySeparatorChar),
            [StringComparison]::OrdinalIgnoreCase
        ) "packaged asset stays inside dist: $reference"
        Assert-True (Test-Path -LiteralPath $assetPath) "packaged asset exists: $reference"
        $references.Add($assetPath)
    }

    foreach ($cssPath in @($references | Where-Object { [IO.Path]::GetExtension($_) -eq '.css' })) {
        $css = Get-Content -LiteralPath $cssPath -Raw
        $urlMatches = [regex]::Matches(
            $css,
            'url\(["'']?(?<path>[^)"'']+)["'']?\)',
            [Text.RegularExpressions.RegexOptions]::IgnoreCase
        )
        foreach ($match in $urlMatches) {
            $reference = $match.Groups['path'].Value
            if ($reference -match '^data:') { continue }
            Assert-True (-not ($reference -match '^(?:https?:|//|javascript:)')) "remote runtime asset: $reference"
            $assetPath = [IO.Path]::GetFullPath((Join-Path (Split-Path $cssPath) $reference))
            Assert-True $assetPath.StartsWith(
                ([IO.Path]::GetFullPath($DistRoot) + [IO.Path]::DirectorySeparatorChar),
                [StringComparison]::OrdinalIgnoreCase
            ) "CSS asset stays inside dist: $reference"
            Assert-True (Test-Path -LiteralPath $assetPath) "packaged CSS asset exists: $reference"
            $references.Add($assetPath)
        }
    }

    Assert-True ($references.Count -ge 3) 'packaged desktop runtime asset count'
    return @($references | Select-Object -Unique)
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
Add-Type -AssemblyName System.Windows.Forms
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
$npmExe = (Get-Command npm.cmd -CommandType Application -ErrorAction Stop).Source
$releaseDir = Join-Path $repoRoot 'target\release'
$script:keeperExe = Join-Path $releaseDir 'lazoo-keeper.exe'
$script:daemonExe = Join-Path $releaseDir 'lazood.exe'
$script:ctlExe = Join-Path $releaseDir 'lazooctl.exe'
$desktopExe = Join-Path $releaseDir 'lazoo-desktop.exe'
$desktopDist = Join-Path $repoRoot 'apps\lazoo-desktop\ui\dist'
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
$commandPaletteScreenshot = Join-Path $runRoot 'desktop-command-palette.png'
$inspectorScreenshot = Join-Path $runRoot 'desktop-inspector.png'
$highDpiScreenshot = Join-Path $runRoot 'desktop-text-scale-200.png'
$textScalePercent = 100
$textScaleSettings = Get-ItemProperty -LiteralPath 'HKCU:\Software\Microsoft\Accessibility' -ErrorAction SilentlyContinue
if ($null -ne $textScaleSettings -and $null -ne $textScaleSettings.TextScaleFactor) {
    $textScalePercent = [int]$textScaleSettings.TextScaleFactor
}
$startedAt = [DateTime]::UtcNow
$stopwatch = [Diagnostics.Stopwatch]::StartNew()
$script:commands = [Collections.Generic.List[object]]::new()
$gates = [ordered]@{}
$keeper = $null
$desktop = $null
$cleanupProbeKeeper = $null
$cleanupProbeOther = $null
$cleanupExitProbe = $null
$manifestStatus = 'FAIL'
$keeperStdout = ''
$keeperStderr = ''
$cleanupErrors = [Collections.Generic.List[string]]::new()
$runtimeAssets = @()

New-Item -ItemType Directory -Force -Path @(
    $artifactDir,
    $script:dataRoot,
    $projectRoot,
    $cleanupOwnedRoot,
    $cleanupUnrelatedRoot
) | Out-Null

try {
    if (-not $SkipBuild) {
        Push-Location $repoRoot
        try {
            $timer = [Diagnostics.Stopwatch]::StartNew()
            & $npmExe --prefix apps/lazoo-desktop/ui ci
            $npmCiExit = $LASTEXITCODE
            $timer.Stop()
            $script:commands.Add([ordered]@{
                command = 'npm --prefix apps/lazoo-desktop/ui ci'
                exitCode = $npmCiExit
                durationMs = $timer.ElapsedMilliseconds
            })
            Assert-Equal $npmCiExit 0 'frontend clean install'

            $timer = [Diagnostics.Stopwatch]::StartNew()
            & $npmExe --prefix apps/lazoo-desktop/ui run build
            $frontendBuildExit = $LASTEXITCODE
            $timer.Stop()
            $script:commands.Add([ordered]@{
                command = 'npm --prefix apps/lazoo-desktop/ui run build'
                exitCode = $frontendBuildExit
                durationMs = $timer.ElapsedMilliseconds
            })
            Assert-Equal $frontendBuildExit 0 'frontend production build'
        } finally {
            Pop-Location
        }
        $gates.frontendBuild = 'PASS'

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
    } else {
        $gates.frontendBuild = 'SKIPPED: prebuilt frontend requested'
    }

    $runtimeAssets = @(Test-DesktopRuntimeAssets $desktopDist)
    $gates.runtimeAssets = 'PASS'
    foreach ($binary in @($script:keeperExe, $script:daemonExe, $script:ctlExe, $desktopExe)) {
        Assert-True (Test-Path -LiteralPath $binary) "binary exists: $binary"
    }
    $gates.releaseBuild = 'PASS'

    $hostExecutable = (Get-Process -Id $PID).Path
    $cleanupExitProbe = Start-HiddenProcess $hostExecutable $cleanupOwnedRoot $true '-NoProfile -Command "Start-Sleep -Milliseconds 250"'
    $null = @(Stop-OwnedProcessTree $cleanupExitProbe.Id 0 500)
    Assert-True $cleanupExitProbe.WaitForExit(5000) 'self-exiting cleanup-probe root stopped'
    Assert-Equal $cleanupExitProbe.ExitCode 0 'cleanup tolerates root exiting before stop request'
    $cleanupExitProbe = $null

    $cleanupProbeKeeper = Start-HiddenProcess $script:keeperExe $cleanupOwnedRoot $true
    $ownedProbeDaemon = Wait-DescendantProcess $cleanupProbeKeeper.Id 'lazood.exe'
    $cleanupProbeOther = Start-HiddenProcess $script:daemonExe $cleanupUnrelatedRoot $true
    Start-Sleep -Milliseconds 500
    Assert-True (-not $cleanupProbeOther.HasExited) 'unrelated cleanup-probe daemon started'

    Assert-True (Stop-OwnedProcess $ownedProbeDaemon) 'cleanup-probe daemon stopped to force restart'
    $observedDuringCleanup = @(Stop-OwnedProcessTree $cleanupProbeKeeper.Id 1500)
    Assert-True $cleanupProbeKeeper.WaitForExit(5000) 'cleanup-probe keeper stopped'
    Assert-True (
        @($observedDuringCleanup | Where-Object { $_ -ne $ownedProbeDaemon.Id }).Count -gt 0
    ) 'cleanup observed a restarted daemon before stopping the keeper'
    Assert-Equal @(Get-DescendantProcessIds $cleanupProbeKeeper.Id).Count 0 'cleanup drained restarted descendants'
    Assert-True (-not $cleanupProbeOther.HasExited) 'cleanup preserves unrelated daemon from same binary'
    $cleanupProbeKeeper = $null
    Assert-True (Stop-OwnedProcess $cleanupProbeOther) 'unrelated cleanup-probe daemon stopped explicitly'
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

    $uiNames = Wait-UiText $windowHandle @(
        'Control plane steady',
        'Open command palette',
        'Minimize window',
        'Maximize or restore window',
        'Close window'
    )
    $projectElement = Wait-AutomationElement $windowHandle 'Desktop acceptance' $true
    $eventElement = Wait-AutomationElement $windowHandle 'Project registered' $true
    Assert-True $eventElement.Current.Name.StartsWith(
        'Project registered',
        [StringComparison]::Ordinal
    ) 'desktop recent event label'
    $gates.snapshotRpc = 'PASS'
    $gates.customTitlebar = 'PASS'

    $commandTrigger = Wait-AutomationElement $windowHandle 'Open command palette'
    Send-WindowKeys $windowHandle '^k' $commandTrigger
    Wait-UiText $windowHandle @('Command palette', 'Search commands') | Out-Null
    Wait-AutomationElement $windowHandle 'Refresh snapshot' $true | Out-Null
    Wait-AutomationElement $windowHandle 'Open projects' $true | Out-Null
    Capture-Window $windowHandle $commandPaletteScreenshot | Out-Null
    $gates.commandPaletteScreenshot = 'PASS'
    Send-WindowKeys $windowHandle '{ESC}'
    Wait-UiTextAbsent $windowHandle @('Search commands')
    $gates.commandPaletteKeyboard = 'PASS'

    $projectElement = Wait-AutomationElement $windowHandle 'Desktop acceptance' $true
    $projectElement.SetFocus()
    Send-WindowKeys $windowHandle '{ENTER}'
    Wait-UiText $windowHandle @('Inspector', 'Close inspector', 'Desktop acceptance') | Out-Null
    Capture-Window $windowHandle $inspectorScreenshot | Out-Null
    $gates.inspectorScreenshot = 'PASS'
    $closeInspector = Wait-AutomationElement $windowHandle 'Close inspector'
    Invoke-AutomationElement $closeInspector
    Wait-UiTextAbsent $windowHandle @('Inspector', 'Close inspector')
    Assert-FocusedAutomationName 'Desktop acceptance' 'focus returned to project row'
    $gates.inspectorFocusRoundTrip = 'PASS'

    Assert-True ([WindowNative]::MoveWindow($windowHandle, 40, 40, 1180, 760, $true)) 'default window resize'
    Start-Sleep -Milliseconds 500
    $defaultBounds = Capture-Window $windowHandle $defaultScreenshot
    Assert-Equal $defaultBounds.Width 1180 'default window width'
    Assert-Equal $defaultBounds.Height 760 'default window height'
    $gates.defaultScreenshot = 'PASS'

    Assert-True ([WindowNative]::MoveWindow($windowHandle, 80, 80, 860, 620, $true)) 'narrow window resize'
    Start-Sleep -Milliseconds 750
    $narrowBounds = Capture-Window $windowHandle $narrowScreenshot
    Assert-Equal $narrowBounds.Width 860 'narrow window width'
    Assert-Equal $narrowBounds.Height 620 'narrow window height'
    $systemNavigation = Wait-AutomationElement $windowHandle 'System'
    Invoke-AutomationElement $systemNavigation
    Wait-UiText $windowHandle @('System pulse') | Out-Null
    $gates.narrowScreenshot = 'PASS'

    if ($textScalePercent -eq 200) {
        Assert-True ([WindowNative]::MoveWindow($windowHandle, 40, 40, 1180, 760, $true)) 'high-DPI window resize'
        Start-Sleep -Milliseconds 500
        $highDpiBounds = Capture-Window $windowHandle $highDpiScreenshot
        Assert-Equal $highDpiBounds.Width 1180 'high-DPI window width'
        Assert-Equal $highDpiBounds.Height 760 'high-DPI window height'
        $gates.highDpiEvidence = 'PASS'
    } else {
        $gates.highDpiEvidence = "BLOCKED: Windows text scaling is $textScalePercent%; 200% is required for this capture"
    }

    Assert-True $desktop.CloseMainWindow() 'desktop accepted close request'
    if (-not $desktop.WaitForExit(5000)) {
        Assert-True (Stop-OwnedProcess $desktop) 'desktop force-close completed'
    }
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
    if ($null -ne $desktop -and -not (Stop-OwnedProcess $desktop)) {
        $cleanupErrors.Add("desktop process $($desktop.Id) did not stop")
    }
    if ($null -ne $cleanupProbeOther -and -not (Stop-OwnedProcess $cleanupProbeOther)) {
        $cleanupErrors.Add("unrelated probe process $($cleanupProbeOther.Id) did not stop")
    }
    if ($null -ne $cleanupExitProbe -and -not (Stop-OwnedProcess $cleanupExitProbe)) {
        $cleanupErrors.Add("self-exiting probe process $($cleanupExitProbe.Id) did not stop")
    }
    if ($null -ne $cleanupProbeKeeper) {
        try { $null = Stop-OwnedProcessTree $cleanupProbeKeeper.Id } catch {
            $cleanupErrors.Add("cleanup-probe keeper tree: $($_.Exception.Message)")
        }
    }
    if ($null -ne $keeper) {
        try { $null = Stop-OwnedProcessTree $keeper.Id } catch {
            $cleanupErrors.Add("main keeper tree: $($_.Exception.Message)")
        }
    }
    if ($cleanupErrors.Count -gt 0) {
        $gates.cleanup = "FAIL: $($cleanupErrors -join '; ')"
        $manifestStatus = 'FAIL'
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
        environment = [ordered]@{
            windowsTextScalePercent = $textScalePercent
        }
        artifacts = [ordered]@{
            manifest = $manifestPath
            runRoot = $runRoot
            defaultScreenshot = $defaultScreenshot
            narrowScreenshot = $narrowScreenshot
            commandPaletteScreenshot = $commandPaletteScreenshot
            inspectorScreenshot = $inspectorScreenshot
            highDpiScreenshot = if (Test-Path -LiteralPath $highDpiScreenshot) {
                $highDpiScreenshot
            } else {
                $null
            }
            runtimeAssets = @($runtimeAssets)
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

if ($manifestStatus -ne 'PASS') {
    throw "desktop shell acceptance cleanup failed: $($cleanupErrors -join '; ')"
}
Write-Output "DESKTOP SHELL ACCEPTANCE: PASS"
Write-Output "PASS manifest: $manifestPath"
Write-Output "Visual QA screenshots: $defaultScreenshot and $narrowScreenshot"
Write-Output "Interaction screenshots: $commandPaletteScreenshot and $inspectorScreenshot"
if (Test-Path -LiteralPath $highDpiScreenshot) {
    Write-Output "High-DPI visual QA screenshot: $highDpiScreenshot"
} else {
    Write-Output $gates.highDpiEvidence
}
