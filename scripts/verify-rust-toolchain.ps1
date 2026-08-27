$ErrorActionPreference = 'Stop'

$repoRoot = Split-Path -Parent $PSScriptRoot
$failures = [System.Collections.Generic.List[string]]::new()

function Assert-Match {
    param(
        [Parameter(Mandatory)]
        [string]$Actual,

        [Parameter(Mandatory)]
        [string]$Pattern,

        [Parameter(Mandatory)]
        [string]$Message
    )

    if ($Actual -notmatch $Pattern) {
        $failures.Add("${Message}. Actual: $Actual")
    }
}

$toolchainConfig = Get-Content -Raw (Join-Path $repoRoot 'rust-toolchain.toml')
$workspaceManifest = Get-Content -Raw (Join-Path $repoRoot 'Cargo.toml')

Assert-Match $toolchainConfig '(?m)^channel = "1\.98\.0"\r?$' 'rust-toolchain.toml must pin Rust 1.98.0'
Assert-Match $workspaceManifest '(?m)^rust-version = "1\.98"\r?$' 'Cargo.toml must declare MSRV 1.98'

$cargoCommand = Get-Command cargo -CommandType Application -ErrorAction Stop
$rustcCommand = Get-Command rustc -CommandType Application -ErrorAction Stop
$cargoDetails = (& $cargoCommand.Source -vV) -join "`n"
$rustcDetails = (& $rustcCommand.Source -vV) -join "`n"

Assert-Match $cargoDetails '(?m)^release: 1\.98\.0$' 'Plain cargo must resolve to version 1.98.0'
Assert-Match $cargoDetails '(?m)^host: x86_64-pc-windows-msvc$' 'Plain cargo must use the MSVC host'
Assert-Match $rustcDetails '(?m)^release: 1\.98\.0$' 'Plain rustc must resolve to version 1.98.0'
Assert-Match $rustcDetails '(?m)^host: x86_64-pc-windows-msvc$' 'Plain rustc must use the MSVC host'

if ($failures.Count -gt 0) {
    $failures | ForEach-Object { Write-Error $_ -ErrorAction Continue }
    exit 1
}

Write-Host "Toolchain verification passed: $($cargoCommand.Source)"
Write-Host (($cargoDetails -split "`n") | Select-Object -First 2)
