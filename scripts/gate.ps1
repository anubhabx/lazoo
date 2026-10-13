param(
    # Skip the cargo steps (format, clippy, tests, release build) and run only the acceptance scripts.
    [switch]$AcceptanceOnly,
    # Pass -SkipBuild to every acceptance script that accepts it (the real-binary ones); the
    # release build this runner performs makes a rebuild inside each script redundant.
    [switch]$SkipBuild = $true,
    # Provider for native-session-ownership.ps1 (a real harness session, no model turn).
    [string]$Provider = 'codex',
    # Acceptance scripts to run (base names); default is every script except the excluded ones.
    [string[]]$Only = @(),
    # Acceptance scripts to skip (base names) in addition to the excluded ones.
    [string[]]$Skip = @(),
    [string]$OutputPath = ''
)

# The repository's full gate, as every backend slice must pass it before its pull request:
# format check, clippy with warnings denied, the workspace tests, the locked release build, and
# every deterministic or real-binary acceptance script. Two scripts are never part of it:
# provider-adapters.ps1 (a live model turn, run only when explicitly authorized) and
# desktop-shell.ps1 (the separately owned desktop). The manifest it writes records every step.

$ErrorActionPreference = 'Stop'
Set-StrictMode -Version Latest

$repoRoot = (Resolve-Path (Join-Path $PSScriptRoot '..')).Path
$cargoExe = (Get-Command cargo -CommandType Application -ErrorAction Stop |
    Select-Object -First 1).Source
$pwshExe = (Get-Command pwsh -CommandType Application -ErrorAction SilentlyContinue |
    Select-Object -First 1).Source
if (-not $pwshExe) {
    $pwshExe = (Get-Command powershell -CommandType Application -ErrorAction Stop |
        Select-Object -First 1).Source
}
$artifactDir = Join-Path $repoRoot 'artifacts\acceptance'
if ([string]::IsNullOrWhiteSpace($OutputPath)) {
    $OutputPath = Join-Path $artifactDir 'gate-manifest.json'
}
$manifestPath = [IO.Path]::GetFullPath($OutputPath)
New-Item -ItemType Directory -Force -Path (Split-Path -Parent $manifestPath) | Out-Null

$excluded = @('provider-adapters', 'desktop-shell')
$startedAt = [DateTime]::UtcNow
$stopwatch = [Diagnostics.Stopwatch]::StartNew()
$steps = [Collections.Generic.List[object]]::new()
$failed = [Collections.Generic.List[string]]::new()
$stopped = $false

function Invoke-Step([string]$Name, [string]$Command, [scriptblock]$Block) {
    Write-Host "==> $Name"
    $timer = [Diagnostics.Stopwatch]::StartNew()
    # Shown, not returned: a function's pipeline output would otherwise ride along with the
    # boolean below and turn a failed step into a truthy array.
    & $Block | Out-Host
    $exitCode = $LASTEXITCODE
    $timer.Stop()
    $steps.Add([ordered]@{
        step = $Name
        command = $Command
        exitCode = $exitCode
        durationMs = [long]$timer.ElapsedMilliseconds
    })
    if ($exitCode -ne 0) {
        $failed.Add($Name)
        Write-Host "FAIL $Name (exit $exitCode)"
    } else {
        Write-Host "PASS $Name ($($timer.ElapsedMilliseconds) ms)"
    }
    return ($exitCode -eq 0)
}

try {
    Push-Location $repoRoot
    if (-not $AcceptanceOnly) {
        # A cargo failure stops the gate: the acceptance scripts would only repeat it.
        $cargoSteps = @(
            @('format', 'cargo fmt --all -- --check', @('fmt', '--all', '--', '--check')),
            @('clippy', 'cargo clippy --workspace --all-targets --locked -- -D warnings',
                @('clippy', '--workspace', '--all-targets', '--locked', '--', '-D', 'warnings')),
            @('test', 'cargo test --workspace --locked', @('test', '--workspace', '--locked')),
            @('release', 'cargo build --workspace --release --locked',
                @('build', '--workspace', '--release', '--locked'))
        )
        foreach ($entry in $cargoSteps) {
            $name, $command, $arguments = $entry
            $ok = Invoke-Step $name $command { & $cargoExe @arguments }
            if (-not $ok) { $stopped = $true; break }
        }
    }

    $scripts = if ($stopped) { @() } else { Get-ChildItem (Join-Path $repoRoot 'scripts\acceptance\*.ps1') |
        Where-Object { $_.BaseName -notin $excluded -and $_.BaseName -notin $Skip } |
        Where-Object { ($Only.Count -eq 0) -or ($_.BaseName -in $Only) } |
        Sort-Object Name }
    foreach ($script in $scripts) {
        $name = $script.BaseName
        $arguments = @()
        $header = Get-Content -LiteralPath $script.FullName -TotalCount 12 | Out-String
        if ($SkipBuild -and $header -match '\[switch\]\$SkipBuild') { $arguments += '-SkipBuild' }
        if ($header -match '\[string\]\$Provider') { $arguments += @('-Provider', $Provider) }
        $command = "$name.ps1 $($arguments -join ' ')".TrimEnd()
        $null = Invoke-Step $name $command {
            & $pwshExe -NoProfile -ExecutionPolicy Bypass -File $script.FullName @arguments
        }
    }
} finally {
    Pop-Location
    $stopwatch.Stop()
    $manifest = [ordered]@{
        status = $(if ($failed.Count -eq 0) { 'PASS' } else { 'FAIL' })
        startedAt = $startedAt.ToString('o')
        durationMs = [long]$stopwatch.ElapsedMilliseconds
        acceptanceOnly = [bool]$AcceptanceOnly
        skipBuild = [bool]$SkipBuild
        provider = $Provider
        excluded = $excluded
        stoppedAtCargoStep = $stopped
        steps = @($steps)
        failed = @($failed)
    }
    $manifest | ConvertTo-Json -Depth 6 | Set-Content -LiteralPath $manifestPath -Encoding utf8
    Write-Host "gate manifest: $manifestPath"
}

if ($failed.Count -gt 0) {
    Write-Host "GATE FAIL: $($failed -join ', ')"
    exit 1
}
Write-Host "GATE PASS: $($steps.Count) steps"
