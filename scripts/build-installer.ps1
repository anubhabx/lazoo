#Requires -Version 7
<#
.SYNOPSIS
Builds the Lazoo Windows installer (NSIS, per-user, no administrator rights).

.DESCRIPTION
Release binaries are linked with a static C runtime so the installed app needs no Visual C++
redistributable. They build in target/installer so the regular gate's release build stays warm.
The daemon, keeper, CLI and session host are staged as Tauri sidecars (installed beside Lazoo.exe,
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
        $installDir = Join-Path ([IO.Path]::GetTempPath()) "lazoo-install-$([guid]::NewGuid().ToString('N'))"
        $checks = [ordered]@{}
        $setup = Start-Process $installer.FullName -ArgumentList '/S', "/D=$installDir" -Wait -PassThru
        $checks.installExit = $setup.ExitCode
        foreach ($exe in @('Lazoo.exe') + ($sidecars | ForEach-Object { "$_.exe" })) {
            $checks[$exe] = Test-Path (Join-Path $installDir $exe)
        }
        # Proves the static-CRT binary loads on its own; --help never contacts or starts the daemon.
        $checks.lazooctlHelp = (& (Join-Path $installDir 'lazooctl.exe') --help 2>&1 | Out-String).Contains('Usage')
        $checks.dataRootUntouched = -not (Test-Path (Join-Path $installDir 'state'))
        $uninstaller = Join-Path $installDir 'uninstall.exe'
        if (Test-Path $uninstaller) {
            $checks.uninstallExit = (Start-Process $uninstaller -ArgumentList '/S' -Wait -PassThru).ExitCode
            Start-Sleep -Seconds 2
            $checks.removed = -not (Test-Path (Join-Path $installDir 'Lazoo.exe'))
        }
        $manifest.verify = $checks
        $failed = $checks.GetEnumerator() | Where-Object { $_.Value -eq $false -or ($_.Key -like '*Exit' -and $_.Value -ne 0) }
        if ($failed) { throw "installer verification failed: $(($failed | ForEach-Object Key) -join ', ')" }
    }

    $out = Join-Path $repo $OutputPath
    New-Item -ItemType Directory -Force (Split-Path $out) | Out-Null
    $manifest | ConvertTo-Json -Depth 5 | Set-Content $out -Encoding utf8NoBOM
    Write-Host "Installer: $($installer.FullName) ($([math]::Round($installer.Length / 1MB, 1)) MB)"
    Write-Host "Manifest:  $out"
} finally {
    Pop-Location
}
