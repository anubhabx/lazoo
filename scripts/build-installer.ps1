#Requires -Version 7
<#
.SYNOPSIS
Builds the Lazoo Windows installer (NSIS, per-user, no administrator rights).

.DESCRIPTION
Release binaries are linked with a static C runtime so the installed app needs no Visual C++
redistributable. They build in target/installer so the regular gate's release build stays warm.
The daemon, keeper, CLI and session host are staged as Tauri sidecars (installed beside lazoo-desktop.exe,
where the keeper and daemon expect each other), then the pinned Tauri CLI bundles the installer
with the vendored template in apps/lazoo-desktop/windows. -Verify silently installs into a
temporary folder, checks every executable starts, and uninstalls again.
#>
param(
    [switch]$Verify,
    [string]$OutputPath = 'artifacts/installer/installer-manifest.json'
)

$ErrorActionPreference = 'Stop'
$repo = Split-Path -Parent $PSScriptRoot
$desktop = Join-Path $repo 'apps/lazoo-desktop'
$triple = 'x86_64-pc-windows-msvc'
$tauriCli = '@tauri-apps/cli@2.11.5' # keep in step with apps/lazoo-desktop/windows/installer.nsi
$sidecars = 'lazood', 'lazoo-keeper', 'lazooctl', 'lazoo-session-host'

$env:CARGO_TARGET_DIR = Join-Path $repo 'target/installer'
$env:RUSTFLAGS = '-C target-feature=+crt-static'

function Invoke-Checked([string]$what, [scriptblock]$command) {
    & $command
    if ($LASTEXITCODE -ne 0) { throw "$what failed with exit code $LASTEXITCODE" }
}

Push-Location $repo
try {
    # The UI bundle is built here: the CLI runs beforeBuildCommand from the frontend folder,
    # where `npm --prefix ui` does not resolve, so the bundle config clears that hook.
    Invoke-Checked 'npm ci (ui)' { npm --prefix (Join-Path $desktop 'ui') ci --no-audit --no-fund }
    Invoke-Checked 'ui build' { npm --prefix (Join-Path $desktop 'ui') run build }

    $packageArgs = $sidecars | ForEach-Object { '-p', $_ }
    Invoke-Checked 'cargo build (sidecars)' { cargo build --release --locked @packageArgs }

    $binaries = New-Item -ItemType Directory -Force (Join-Path $desktop 'binaries')
    foreach ($name in $sidecars) {
        Copy-Item (Join-Path $env:CARGO_TARGET_DIR "release/$name.exe") `
            (Join-Path $binaries "$name-$triple.exe") -Force
    }

    Push-Location $desktop
    try {
        # Bundle settings live in tauri.bundle.conf.json: tauri-build checks externalBin paths at
        # compile time, so the main config must not name sidecars that ordinary builds never stage.
        Invoke-Checked 'tauri build' {
            npx --yes $tauriCli build --ci --bundles nsis --config tauri.bundle.conf.json -- --locked
        }
    } finally {
        Pop-Location
    }

    $installer = Get-ChildItem (Join-Path $env:CARGO_TARGET_DIR 'release/bundle/nsis') -Filter '*-setup.exe' |
        Sort-Object LastWriteTime | Select-Object -Last 1
    if (-not $installer) { throw 'tauri build produced no NSIS installer' }

    $manifest = [ordered]@{
        builtAt   = (Get-Date).ToUniversalTime().ToString('o')
        commit    = (git rev-parse HEAD).Trim()
        installer = [ordered]@{
            path   = [IO.Path]::GetRelativePath($repo, $installer.FullName)
            bytes  = $installer.Length
            sha256 = (Get-FileHash $installer.FullName -Algorithm SHA256).Hash.ToLowerInvariant()
        }
        tauriCli  = $tauriCli
        rustflags = $env:RUSTFLAGS
        verify    = $null
    }

    if ($Verify) {
        # Start-Process -Wait can report a null ExitCode unless the handle is held first.
        function Invoke-Exit([string]$file, [string[]]$arguments) {
            $process = Start-Process $file -ArgumentList $arguments -PassThru
            $null = $process.Handle
            $process.WaitForExit()
            $process.ExitCode
        }
        function Get-InstalledService([string]$dir) {
            @(Get-Process lazoo-keeper, lazood -ErrorAction SilentlyContinue |
                Where-Object { $_.Path -like "$dir\*" })
        }
        $stamp = [guid]::NewGuid().ToString('N').Substring(0, 12)
        $installDir = Join-Path ([IO.Path]::GetTempPath()) "lazoo-install-$stamp"
        # The launched app must never touch the owner's real %LOCALAPPDATA%\Lazoo.
        $verifyRoot = Join-Path ([IO.Path]::GetTempPath()) "lazoo-verify-data-$stamp"
        $checks = [ordered]@{}
        $checks.installExit = Invoke-Exit $installer.FullName @('/S', "/D=$installDir")
        foreach ($exe in @('lazoo-desktop.exe') + ($sidecars | ForEach-Object { "$_.exe" })) {
            $checks[$exe] = Test-Path (Join-Path $installDir $exe)
        }
        # Proves the static-CRT binary loads on its own; --help never contacts or starts the daemon.
        $checks.lazooctlHelp = (& (Join-Path $installDir 'lazooctl.exe') --help 2>&1 | Out-String).Contains('Usage')

        $previousRoot = $env:LAZOO_DATA_ROOT
        $env:LAZOO_DATA_ROOT = $verifyRoot
        try {
            # The installed app starts its own keeper, which starts the daemon, all from $installDir.
            $app = Start-Process (Join-Path $installDir 'lazoo-desktop.exe') -PassThru
            $deadline = (Get-Date).AddSeconds(40)
            do { Start-Sleep -Milliseconds 500; $service = Get-InstalledService $installDir }
            until ($service.Count -ge 2 -or (Get-Date) -gt $deadline)
            $checks.serviceStartedByApp = $service.Count -ge 2
            # The pipe opens a moment after the processes appear.
            $statusDeadline = (Get-Date).AddSeconds(20)
            do {
                & (Join-Path $installDir 'lazooctl.exe') status *> $null
                $checks.statusExit = $LASTEXITCODE
                if ($checks.statusExit -ne 0) { Start-Sleep -Milliseconds 500 }
            } until ($checks.statusExit -eq 0 -or (Get-Date) -gt $statusDeadline)
            Stop-Process -Id $app.Id -Force -ErrorAction SilentlyContinue
            # The uninstall hook stops the installed service before removing files.
            $checks.uninstallExit = Invoke-Exit (Join-Path $installDir 'uninstall.exe') @('/S')
            Start-Sleep -Seconds 3
            $checks.serviceStoppedByUninstall = (Get-InstalledService $installDir).Count -eq 0
            $checks.removed = -not (Test-Path (Join-Path $installDir 'lazoo-desktop.exe'))
        } finally {
            $env:LAZOO_DATA_ROOT = $previousRoot
            Get-InstalledService $installDir | Stop-Process -Force -ErrorAction SilentlyContinue
        }
        $manifest.verify = $checks
        $failed = @($checks.GetEnumerator() | Where-Object { ($_.Value -is [bool] -and -not $_.Value) -or ($_.Key -like '*Exit' -and $_.Value -ne 0) })
    }

    $out = Join-Path $repo $OutputPath
    New-Item -ItemType Directory -Force (Split-Path $out) | Out-Null
    $manifest | ConvertTo-Json -Depth 5 | Set-Content $out -Encoding utf8NoBOM
    if ($Verify -and $failed) {
        throw "installer verification failed: $(($failed | ForEach-Object { "$($_.Key)=$($_.Value)" }) -join ', ')"
    }
    Write-Host "Installer: $($installer.FullName) ($([math]::Round($installer.Length / 1MB, 1)) MB)"
    Write-Host "Manifest:  $out"
} finally {
    Pop-Location
}
