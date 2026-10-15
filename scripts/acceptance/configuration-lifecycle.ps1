param(
    [switch]$SkipBuild
)

$ErrorActionPreference = 'Stop'
Set-StrictMode -Version Latest

function Get-Sha256([string]$Path) {
    return (Get-FileHash -LiteralPath $Path -Algorithm SHA256).Hash.ToLowerInvariant()
}

function Get-ComparisonPath([string]$Path) {
    $absolute = [IO.Path]::GetFullPath($Path)
    if ($absolute.StartsWith('\\?\')) { $absolute = $absolute.Substring(4) }
    return $absolute
}

function Invoke-Ctl([string[]]$Arguments) {
    $timer = [Diagnostics.Stopwatch]::StartNew()
    $previousPreference = $ErrorActionPreference
    try {
        $ErrorActionPreference = 'Continue'
        $lines = @(& $script:ctlExe --json @Arguments 2>&1)
        $exitCode = $LASTEXITCODE
    } finally {
        $ErrorActionPreference = $previousPreference
        $timer.Stop()
    }
    $script:commands.Add([ordered]@{
        command = "lazooctl --json $($Arguments -join ' ')"
        exitCode = $exitCode
        durationMs = [long]$timer.ElapsedMilliseconds
    })
    [pscustomobject]@{ ExitCode = $exitCode; Text = ($lines -join [Environment]::NewLine) }
}

function Invoke-CtlJson([string[]]$Arguments) {
    $result = Invoke-Ctl $Arguments
    if ($result.ExitCode -ne 0) { throw "lazooctl failed: $($result.Text)" }
    try { return $result.Text | ConvertFrom-Json } catch { throw "lazooctl did not return JSON: $($result.Text)" }
}

function Wait-Daemon() {
    $deadline = [DateTime]::UtcNow.AddSeconds(20)
    do {
        $result = Invoke-Ctl @('status')
        if ($result.ExitCode -eq 0) { return $result.Text | ConvertFrom-Json }
        Start-Sleep -Milliseconds 100
    } while ([DateTime]::UtcNow -lt $deadline)
    throw 'daemon did not become ready'
}

function Assert-Equal([object]$Actual, [object]$Expected, [string]$Message) {
    if ($Actual -cne $Expected) { throw "$Message; actual='$Actual' expected='$Expected'" }
}

$repoRoot = (Resolve-Path (Join-Path $PSScriptRoot '..\..')).Path
$releaseDir = Join-Path $repoRoot 'target\release'
$script:daemonExe = Join-Path $releaseDir 'lazood.exe'
$script:ctlExe = Join-Path $releaseDir 'lazooctl.exe'
$artifactDir = Join-Path $repoRoot 'artifacts\acceptance'
$manifestPath = Join-Path $artifactDir 'configuration-lifecycle-manifest.json'
$runId = [Guid]::NewGuid().ToString('N')
$runRoot = Join-Path ([IO.Path]::GetTempPath()) "lazoo-configuration-lifecycle-$runId"
$dataRoot = Join-Path $runRoot 'data'
$codexHome = Join-Path $runRoot 'codex-home'
$configPath = Join-Path $codexHome 'config.toml'
$stdoutLog = Join-Path $runRoot 'lazood.stdout.log'
$stderrLog = Join-Path $runRoot 'lazood.stderr.log'
$script:commands = [Collections.Generic.List[object]]::new()
$startedAt = [DateTime]::UtcNow
$timer = [Diagnostics.Stopwatch]::StartNew()
$daemon = $null
$previousDataRoot = $env:LAZOO_DATA_ROOT
$previousCodexHome = $env:CODEX_HOME
$status = 'FAIL'
$failure = $null
$manifest = $null

New-Item -ItemType Directory -Force -Path $artifactDir, $dataRoot, $codexHome | Out-Null

try {
    if (-not $SkipBuild) {
        & cargo build --workspace --release --locked
        if ($LASTEXITCODE -ne 0) { throw "release build exited $LASTEXITCODE" }
    }
    if (-not (Test-Path -LiteralPath $script:daemonExe -PathType Leaf) -or
        -not (Test-Path -LiteralPath $script:ctlExe -PathType Leaf)) {
        throw 'release lazood/lazooctl binaries are required'
    }

    $env:LAZOO_DATA_ROOT = $dataRoot
    $env:CODEX_HOME = $codexHome
    $desktopCodex = Get-ChildItem -LiteralPath (Join-Path $env:LOCALAPPDATA 'OpenAI\Codex\bin') -Filter codex.exe -Recurse -File |
        ForEach-Object {
            $version = (& $_.FullName --version 2>$null | Where-Object { $_ -match '^codex-cli ' } | Select-Object -First 1).ToString().Trim()
            [pscustomobject]@{ Path = $_.FullName; Version = $version }
        } |
        Where-Object { $_.Version -eq 'codex-cli 0.162.0-alpha.17.2' } |
        Select-Object -First 1
    if ($null -eq $desktopCodex) { throw 'exact installed Codex 0.162.0-alpha.17.2 was not found' }

    @'
model = "fixture-model"

[mcp_servers.preexisting]
command = "fixture-existing"
args = ["--preserve"]
enabled = false
'@ | Set-Content -LiteralPath $configPath -Encoding utf8NoBOM
    $originalBytes = [IO.File]::ReadAllBytes($configPath)
    $originalHash = Get-Sha256 $configPath

    # Both the daemon and the read-only installed-Codex version probe inherit this fixture only.
    $observedVersion = (& $desktopCodex.Path --version 2>$null | Where-Object { $_ -match '^codex-cli ' } | Select-Object -First 1).ToString().Trim()
    Assert-Equal $observedVersion 'codex-cli 0.162.0-alpha.17.2' 'unexpected installed Codex version'

    $daemon = Start-Process -FilePath $script:daemonExe -WorkingDirectory $repoRoot -WindowStyle Hidden -PassThru -RedirectStandardOutput $stdoutLog -RedirectStandardError $stderrLog
    $null = Wait-Daemon

    $selection = [ordered]@{
        provider = 'codex'
        name = 'acceptance-fixture'
        command = 'fixture-mcp'
        args = @('--no-connect')
        url = $null
        enabled = $false
        replace = $false
        selectedSecretFields = @()
    } | ConvertTo-Json -Compress
    $preview = Invoke-CtlJson @('provider', 'config-preview', '--selection', $selection)
    $expectedTarget = Get-ComparisonPath $configPath
    $actualTarget = Get-ComparisonPath ([string]$preview.preview.target)
    Assert-Equal $actualTarget $expectedTarget 'preview target is outside the disposable CODEX_HOME'
    Assert-Equal ([string]$preview.preview.originalHash).ToLowerInvariant() $originalHash 'preview original hash changed before apply'
    Assert-Equal (Get-Sha256 $configPath) $originalHash 'preview wrote the fixture config'

    $operationId = [string]$preview.id
    $previewRevision = [string]$preview.revision
    $apply = Invoke-CtlJson @('provider', 'config-apply', $operationId, '--expected-revision', $previewRevision)
    Assert-Equal ([string]$apply.state) 'applied' 'first apply did not finalize'
    $afterApply = Get-Content -LiteralPath $configPath -Raw
    if ($afterApply -notmatch 'model\s*=\s*"fixture-model"' -or
        $afterApply -notmatch '\[mcp_servers\.preexisting\]' -or
        $afterApply -notmatch '\[mcp_servers\.acceptance-fixture\]' -or
        $afterApply -notmatch 'enabled\s*=\s*false') {
        throw 'apply did not preserve unrelated fixture configuration or add the disabled entry'
    }
    if ((Get-Sha256 $configPath) -eq $originalHash) { throw 'apply did not change the fixture config' }

    # The original preview revision deliberately exercises a lost-reply replay after apply advanced it.
    $replay = Invoke-CtlJson @('provider', 'config-apply', $operationId, '--expected-revision', $previewRevision)
    Assert-Equal ([string]$replay.state) 'applied' 'lost-reply replay was not idempotent'
    Assert-Equal ([string]$replay.revision) ([string]$apply.revision) 'lost-reply replay changed operation revision'

    $shown = Invoke-CtlJson @('provider', 'config-show', $operationId)
    Assert-Equal ([string]$shown.state) 'applied' 'show did not return applied operation'
    Assert-Equal (Get-ComparisonPath ([string]$shown.target)) $expectedTarget 'show target differs from preview target'

    $rollback = Invoke-CtlJson @('provider', 'config-rollback', $operationId, '--expected-revision', ([string]$shown.revision))
    Assert-Equal ([string]$rollback.state) 'rolled-back' 'rollback did not finalize'
    $restoredBytes = [IO.File]::ReadAllBytes($configPath)
    if (-not [Linq.Enumerable]::SequenceEqual[byte]($originalBytes, $restoredBytes)) {
        throw 'rollback did not restore the exact original fixture bytes'
    }
    Assert-Equal (Get-Sha256 $configPath) $originalHash 'rollback hash differs from preview original hash'
    $status = 'PASS'
} catch {
    $failure = $_.Exception.Message
} finally {
    $timer.Stop()
    if ($null -ne $daemon -and -not $daemon.HasExited) {
        $shutdown = Invoke-Ctl @('system', 'shutdown')
        if (-not $daemon.WaitForExit(5000)) { Stop-Process -Id $daemon.Id -Force }
    }
    $env:LAZOO_DATA_ROOT = $previousDataRoot
    $env:CODEX_HOME = $previousCodexHome
    $manifest = [ordered]@{
        status = $status
        startedAt = $startedAt.ToString('o')
        completedAt = [DateTime]::UtcNow.ToString('o')
        durationMs = [long]$timer.ElapsedMilliseconds
        binarySha256 = [ordered]@{
            daemon = if (Test-Path -LiteralPath $script:daemonExe -PathType Leaf) { Get-Sha256 $script:daemonExe } else { $null }
            cli = if (Test-Path -LiteralPath $script:ctlExe -PathType Leaf) { Get-Sha256 $script:ctlExe } else { $null }
        }
        codex = [ordered]@{ version = 'codex-cli 0.162.0-alpha.17.2'; probe = 'version and daemon app-server handshake; no model' }
        lifecycle = [ordered]@{ preview = $status -eq 'PASS'; apply = $status -eq 'PASS'; lostReplyReplay = $status -eq 'PASS'; show = $status -eq 'PASS'; rollback = $status -eq 'PASS' }
        boundaries = [ordered]@{ codexHome = 'disposable'; dataRoot = 'disposable'; realProviderProfile = 'NOT_READ_OR_WRITTEN'; externalMcpConnection = 'NOT_STARTED'; modelRequest = 'NOT_RUN'; nativeAuth = 'NOT_REQUIRED' }
        commands = @($script:commands)
        failure = $failure
    }
    $manifest | ConvertTo-Json -Depth 8 | Set-Content -LiteralPath $manifestPath -Encoding utf8
    if (Test-Path -LiteralPath $runRoot) {
        $resolvedRunRoot = (Resolve-Path -LiteralPath $runRoot).Path
        $expectedTempRoot = [IO.Path]::GetFullPath([IO.Path]::GetTempPath()).TrimEnd('\', '/')
        if ([IO.Path]::GetDirectoryName($resolvedRunRoot) -eq $expectedTempRoot -and
            [IO.Path]::GetFileName($resolvedRunRoot) -eq "lazoo-configuration-lifecycle-$runId") {
            Remove-Item -LiteralPath $resolvedRunRoot -Recurse -Force -ErrorAction SilentlyContinue
        }
    }
}

if ($status -ne 'PASS') { throw "configuration lifecycle acceptance failed; evidence: $manifestPath; $failure" }
Write-Output "configuration lifecycle acceptance passed: $manifestPath"
