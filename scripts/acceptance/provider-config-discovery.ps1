param(
    [string]$OutputPath = ''
)

$ErrorActionPreference = 'Stop'
Set-StrictMode -Version Latest

$repoRoot = (Resolve-Path (Join-Path $PSScriptRoot '..\..')).Path
$cargoExe = (Get-Command cargo -CommandType Application -ErrorAction Stop |
    Select-Object -First 1).Source
$gitExe = (Get-Command git -CommandType Application -ErrorAction Stop |
    Select-Object -First 1).Source
$artifactDir = Join-Path $repoRoot 'artifacts\acceptance'
if ([string]::IsNullOrWhiteSpace($OutputPath)) {
    $OutputPath = Join-Path $artifactDir 'provider-config-discovery-manifest.json'
}
$manifestPath = [IO.Path]::GetFullPath($OutputPath)
$manifestDirectory = Split-Path -Parent $manifestPath
New-Item -ItemType Directory -Force -Path $manifestDirectory | Out-Null

$startedAt = [DateTime]::UtcNow
$stopwatch = [Diagnostics.Stopwatch]::StartNew()
$commands = [Collections.Generic.List[object]]::new()
$manifestStatus = 'FAIL'
$evidence = [ordered]@{
    knownFormatsParsed = $false
    secretsRedacted = $false
    integrationsHashed = $false
    duplicatesAndConflictsNamed = $false
    unsupportedNamed = $false
    malformedFilesReportedNotQuoted = $false
    codexHomeHonoured = $false
    cliMapsConfig = $false
    daemonServesConfig = $false
    nothingWritten = $false
    snapshotUnchanged = $false
    schemaFifteen = $false
    providerSmokeAbsent = $true
    externalModelUseAbsent = $true
    uiChangesAbsent = $false
}

function Invoke-Test([string]$Package, [string]$Target, [string]$Name) {
    $arguments = @('test', '-p', $Package, '--test', $Target, $Name, '--locked', '--', '--exact')
    $timer = [Diagnostics.Stopwatch]::StartNew()
    & $cargoExe @arguments
    $exitCode = $LASTEXITCODE
    $timer.Stop()
    $commands.Add([ordered]@{
        command = "cargo $($arguments -join ' ')"
        exitCode = $exitCode
        durationMs = [long]$timer.ElapsedMilliseconds
    })
    if ($exitCode -ne 0) {
        throw "cargo test failed: $Name"
    }
}

try {
    Push-Location $repoRoot
    Invoke-Test 'lazoo-providers' 'config' 'provider_configuration_is_parsed_from_known_formats_redacted_hashed_and_compared'
    $evidence.knownFormatsParsed = $true
    $evidence.secretsRedacted = $true
    $evidence.integrationsHashed = $true
    $evidence.duplicatesAndConflictsNamed = $true
    $evidence.unsupportedNamed = $true

    Invoke-Test 'lazoo-providers' 'config' 'missing_and_malformed_configuration_is_reported_without_quoting_it_or_failing'
    $evidence.malformedFilesReportedNotQuoted = $true
    $evidence.codexHomeHonoured = $true

    Invoke-Test 'lazooctl' 'cli' 'provider_cli_maps_config'
    $evidence.cliMapsConfig = $true

    Invoke-Test 'lazood' 'daemon' 'provider_config_reports_redacted_integrations_without_changing_the_desktop_snapshot'
    $evidence.daemonServesConfig = $true
    $evidence.nothingWritten = $true
    $evidence.snapshotUnchanged = $true

    Invoke-Test 'lazoo-store' 'store' 'store_enables_wal_foreign_keys_and_schema_version'
    $evidence.schemaFifteen = $true

    & $gitExe diff --quiet -- apps/lazoo-desktop
    $unstagedExit = $LASTEXITCODE
    & $gitExe diff --cached --quiet -- apps/lazoo-desktop
    $stagedExit = $LASTEXITCODE
    $commands.Add([ordered]@{
        command = 'git diff --quiet and git diff --cached --quiet -- apps/lazoo-desktop'
        exitCode = [Math]::Max($unstagedExit, $stagedExit)
        durationMs = $null
    })
    if (($unstagedExit -ne 0) -or ($stagedExit -ne 0)) {
        throw 'desktop UI changes are outside the provider config discovery slice'
    }
    $evidence.uiChangesAbsent = $true
    $manifestStatus = 'PASS'
} finally {
    Pop-Location
    $stopwatch.Stop()
    $manifest = [ordered]@{
        status = $manifestStatus
        schemaVersion = 21
        mode = 'deterministic-fake-transport'
        startedAt = $startedAt.ToString('o')
        durationMs = [long]$stopwatch.ElapsedMilliseconds
        commands = @($commands)
        evidence = $evidence
        boundaries = [ordered]@{
            providerSmoke = 'NOT_RUN'
            externalModelUse = 'NOT_RUN'
            desktopUi = 'UNCHANGED'
            schemaMigration = 'NONE'
            writes = 'NONE'
            formats = 'CODEX_TOML_CLAUDE_JSON_OPENCODE_JSON'
            secrets = 'KEY_NAMES_ONLY_URL_QUERY_STRIPPED'
            federationSteps = 'IMPORT_1_TO_5_ONLY'
            toolAccess = 'NONE'
        }
    }
    $manifest | ConvertTo-Json -Depth 8 | Set-Content -LiteralPath $manifestPath -Encoding utf8
}

Write-Host "PASS manifest: $manifestPath"
