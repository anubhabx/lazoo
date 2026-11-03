param(
    [switch]$SkipBuild,
    [string]$OutputPath = ''
)

$ErrorActionPreference = 'Stop'
Set-StrictMode -Version Latest

$repoRoot = (Resolve-Path (Join-Path $PSScriptRoot '..\..')).Path
$cargoExe = (Get-Command cargo -CommandType Application -ErrorAction Stop | Select-Object -First 1).Source
$artifactDir = Join-Path $repoRoot 'artifacts\acceptance'
if ([string]::IsNullOrWhiteSpace($OutputPath)) {
    $OutputPath = Join-Path $artifactDir 'skills-plugins-manifest.json'
}
$manifestPath = [IO.Path]::GetFullPath($OutputPath)
New-Item -ItemType Directory -Force -Path (Split-Path -Parent $manifestPath) | Out-Null

$startedAt = [DateTime]::UtcNow
$timer = [Diagnostics.Stopwatch]::StartNew()
$commands = [Collections.Generic.List[object]]::new()
$status = 'FAIL'
$env:CARGO_BUILD_JOBS = '2'
$env:CARGO_INCREMENTAL = '0'
$env:CARGO_PROFILE_DEV_DEBUG = '0'
$env:CARGO_PROFILE_TEST_DEBUG = '0'
$env:RUST_TEST_THREADS = '2'

function Invoke-FocusedTest([string]$Package, [string]$Target, [string]$Name) {
    $arguments = @('test', '-p', $Package)
    if ($Target -eq '--lib') { $arguments += '--lib' } else { $arguments += @('--test', $Target) }
    if (-not [string]::IsNullOrWhiteSpace($Name)) { $arguments += $Name }
    $arguments += '--locked'
    if (($Target -ne '--lib') -and -not [string]::IsNullOrWhiteSpace($Name)) { $arguments += @('--', '--exact') }
    $commandTimer = [Diagnostics.Stopwatch]::StartNew()
    & $cargoExe @arguments
    $exitCode = $LASTEXITCODE
    $commandTimer.Stop()
    $commands.Add([ordered]@{ command = "cargo $($arguments -join ' ')"; exitCode = $exitCode; durationMs = [long]$commandTimer.ElapsedMilliseconds })
    if ($exitCode -ne 0) { throw "focused test failed: $Package/$Target::$Name" }
}

try {
    Push-Location $repoRoot
    if (-not $SkipBuild) {
        $buildTimer = [Diagnostics.Stopwatch]::StartNew()
        & $cargoExe build -p lazoo-session-host --locked
        $buildExit = $LASTEXITCODE
        $commands.Add([ordered]@{ command = 'cargo build -p lazoo-session-host --locked'; exitCode = $buildExit; durationMs = [long]$buildTimer.ElapsedMilliseconds })
        if ($buildExit -ne 0) { throw 'native command test host build failed' }
    }
    Invoke-FocusedTest 'lazood' 'skills_plugins' ''
    Invoke-FocusedTest 'lazooctl' 'cli' 'skills_and_plugins_cli_map_public_extension_requests'
    Invoke-FocusedTest 'lazoo-providers' '--lib' 'extensions'
    Invoke-FocusedTest 'lazoo-store' 'store' 'store_enables_wal_foreign_keys_and_schema_version'
    $status = 'PASS'
} finally {
    Pop-Location
    $timer.Stop()
    $manifest = [ordered]@{
        manifestVersion = 1
        status = $status
        schemaVersion = 23
        mode = 'deterministic-temporary-homes'
        startedAt = $startedAt.ToString('o')
        durationMs = [long]$timer.ElapsedMilliseconds
        commands = @($commands)
        binaries = [ordered]@{
            cargo = $cargoExe
            sessionHost = $(
                $hostPath = Join-Path $repoRoot 'target/debug/lazoo-session-host.exe'
                if (Test-Path -LiteralPath $hostPath) {
                    [ordered]@{ path = $hostPath; sha256 = (Get-FileHash -Algorithm SHA256 -LiteralPath $hostPath).Hash }
                }
            )
        }
        sourceHashes = @(
            'apps/lazood/tests/skills_plugins.rs',
            'apps/lazooctl/tests/cli.rs',
            'crates/lazoo-providers/src/extensions/plugins.rs',
            'migrations/0023_extensions.sql'
        ) | ForEach-Object {
            $path = Join-Path $repoRoot $_
            [ordered]@{ path = $_; sha256 = (Get-FileHash -Algorithm SHA256 -LiteralPath $path).Hash }
        }
        environment = [ordered]@{ CARGO_BUILD_JOBS = '2'; CARGO_INCREMENTAL = '0'; CARGO_PROFILE_DEV_DEBUG = '0'; CARGO_PROFILE_TEST_DEBUG = '0'; RUST_TEST_THREADS = '2' }
        assertions = @('temporary-home-only', 'revision-fence', 'unmanaged-preservation', 'copy-drift-refusal', 'link-lifecycle', 'update-staging-tamper', 'native-profile-expansion', 'pending-intent-block', 'fake-native-plugin-preview-apply', 'partial-plugin-outcomes', 'cli-request-mapping', 'provider-extension-units', 'schema-23')
        boundaries = [ordered]@{ realNativeHarness = 'NOT_RUN'; userHomes = 'UNTOUCHED'; network = 'NOT_USED' }
    }
    $manifest | ConvertTo-Json -Depth 8 | Set-Content -LiteralPath $manifestPath -Encoding utf8
}

if ($status -ne 'PASS') { exit 1 }
Write-Host "PASS manifest: $manifestPath"
