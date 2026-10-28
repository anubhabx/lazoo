param(
    [Parameter(Mandatory = $true)]
    [ValidateSet('codex', 'opencode', 'grok-build', 'antigravity', 'claude-code')]
    [string]$Provider,
    [switch]$HandshakeOnly,
    [switch]$SkipBuild
)

$ErrorActionPreference = 'Stop'
Set-StrictMode -Version Latest

# Owner scope update, 2026-10-16: Grok is excluded from core v1 acceptance.
# Retain its historical evidence, but do not let it satisfy or block a required gate.
$requiredCoreProviders = @('codex', 'opencode', 'antigravity', 'claude-code')

function Get-CoreProviderStatus([Collections.IDictionary]$Entries, [string[]]$RequiredProviders) {
    if (@($RequiredProviders | Where-Object { -not $Entries.Contains($_) }).Count -gt 0) {
        return 'PARTIAL'
    }
    $statuses = @($RequiredProviders | ForEach-Object { $Entries[$_].status })
    if ($statuses -contains 'FAIL') { return 'FAIL' }
    if ($statuses -contains 'BLOCKED') { return 'BLOCKED' }
    if (@($statuses | Where-Object { $_ -ne 'PASS' }).Count -gt 0) { return 'UNVERIFIED' }
    return 'PASS'
}

function ConvertTo-PathHash([string]$Path) {
    $sha256 = [Security.Cryptography.SHA256]::Create()
    try {
        $bytes = [Text.Encoding]::UTF8.GetBytes($Path.ToLowerInvariant())
        return ([BitConverter]::ToString($sha256.ComputeHash($bytes))).Replace('-', '').ToLowerInvariant()
    } finally {
        $sha256.Dispose()
    }
}

function ConvertTo-SafeBlocker([string]$Message) {
    if ([string]::IsNullOrWhiteSpace($Message)) { return $null }
    $safe = $Message -replace '(?i)(api[_-]?key|token|authorization|secret|password)\s*[:=]\s*\S+', '$1=<redacted>'
    $safe = $safe -replace '[A-Z0-9._%+-]+@[A-Z0-9.-]+\.[A-Z]{2,}', '<redacted-email>'
    if ($safe.Length -gt 512) { $safe = $safe.Substring(0, 512) }
    return $safe
}

function Invoke-Ctl([string[]]$Arguments) {
    $timer = [Diagnostics.Stopwatch]::StartNew()
    $previousPreference = $ErrorActionPreference
    try {
        $ErrorActionPreference = 'Continue'
        $captured = @(& $script:ctlExe --json @Arguments 2>&1)
        $exitCode = $LASTEXITCODE
    } finally {
        $ErrorActionPreference = $previousPreference
        $timer.Stop()
    }
    $loggedArguments = @($Arguments)
    $promptIndex = [Array]::IndexOf($loggedArguments, '--prompt')
    if ($promptIndex -ge 0 -and ($promptIndex + 1) -lt $loggedArguments.Count) {
        $loggedArguments[$promptIndex + 1] = '<conformance-prompt>'
    }
    $script:commands.Add([ordered]@{
        command = "lazooctl --json $($loggedArguments -join ' ')"
        exitCode = $exitCode
        durationMs = $timer.ElapsedMilliseconds
    })
    return [pscustomobject]@{
        ExitCode = $exitCode
        Text = ($captured -join [Environment]::NewLine)
    }
}

function Wait-Daemon([int]$TimeoutSeconds = 20) {
    $deadline = [DateTime]::UtcNow.AddSeconds($TimeoutSeconds)
    $lastError = ''
    while ([DateTime]::UtcNow -lt $deadline) {
        $result = Invoke-Ctl @('status')
        if ($result.ExitCode -eq 0) { return ($result.Text | ConvertFrom-Json) }
        $lastError = $result.Text
        Start-Sleep -Milliseconds 100
    }
    throw "daemon did not become ready: $(ConvertTo-SafeBlocker $lastError)"
}

function Read-ExistingProviders([string]$Path) {
    $providers = [ordered]@{}
    if (-not (Test-Path -LiteralPath $Path)) { return $providers }
    try {
        $existing = Get-Content -LiteralPath $Path -Raw | ConvertFrom-Json
        if ($null -ne $existing.providers) {
            foreach ($property in $existing.providers.PSObject.Properties) {
                $providers[$property.Name] = $property.Value
            }
        }
    } catch {
        # A stale or partial generated manifest is replaced by fresh evidence.
    }
    return $providers
}

$repoRoot = (Resolve-Path (Join-Path $PSScriptRoot '..\..')).Path
$cargoExe = (Get-Command cargo -CommandType Application -ErrorAction Stop).Source
$releaseDir = Join-Path $repoRoot 'target\release'
$script:daemonExe = Join-Path $releaseDir 'lazood.exe'
$script:ctlExe = Join-Path $releaseDir 'lazooctl.exe'
$artifactDir = Join-Path $repoRoot 'artifacts\acceptance'
$manifestPath = Join-Path $artifactDir 'provider-adapters-manifest.json'
$runId = [Guid]::NewGuid().ToString('N')
$runRoot = Join-Path ([System.IO.Path]::GetTempPath()) "lazoo-provider-acceptance-$runId"
$dataRoot = Join-Path $runRoot 'data'
$stdoutLog = Join-Path $runRoot 'lazood.stdout.log'
$stderrLog = Join-Path $runRoot 'lazood.stderr.log'
$startedAt = [DateTime]::UtcNow
$timer = [Diagnostics.Stopwatch]::StartNew()
$script:commands = [Collections.Generic.List[object]]::new()
$daemon = $null
$previousDataRoot = $env:LAZOO_DATA_ROOT
$entry = $null

$expectations = @{
    'codex' = [pscustomobject]@{ Prompt = 'Reply with exactly LAZOO_CODEX_OK. Do not use tools.'; Expected = 'LAZOO_CODEX_OK' }
    'opencode' = [pscustomobject]@{ Prompt = 'Reply with exactly LAZOO_OPENCODE_OK. Do not use tools.'; Expected = 'LAZOO_OPENCODE_OK' }
    'grok-build' = [pscustomobject]@{ Prompt = 'Reply with exactly LAZOO_GROK_OK. Do not use tools.'; Expected = 'LAZOO_GROK_OK' }
    'antigravity' = [pscustomobject]@{ Prompt = 'Reply with exactly LAZOO_AGY_OK. Do not use tools.'; Expected = 'LAZOO_AGY_OK' }
    'claude-code' = [pscustomobject]@{ Prompt = 'Reply with exactly LAZOO_CLAUDE_OK. Do not use tools.'; Expected = 'LAZOO_CLAUDE_OK' }
}

New-Item -ItemType Directory -Force -Path $artifactDir, $dataRoot | Out-Null

try {
    if (-not $SkipBuild) {
        $buildTimer = [Diagnostics.Stopwatch]::StartNew()
        & $cargoExe build --workspace --release --locked
        $buildExit = $LASTEXITCODE
        $buildTimer.Stop()
        $script:commands.Add([ordered]@{
            command = 'cargo build --workspace --release --locked'
            exitCode = $buildExit
            durationMs = $buildTimer.ElapsedMilliseconds
        })
        if ($buildExit -ne 0) { throw "release provider binaries failed to build with exit $buildExit" }
    }
    if (-not (Test-Path -LiteralPath $script:daemonExe) -or -not (Test-Path -LiteralPath $script:ctlExe)) {
        throw 'release provider binaries were not found after build'
    }

    $env:LAZOO_DATA_ROOT = $dataRoot
    $daemon = Start-Process -FilePath $script:daemonExe -WorkingDirectory $repoRoot -WindowStyle Hidden -PassThru -RedirectStandardOutput $stdoutLog -RedirectStandardError $stderrLog
    $status = Wait-Daemon
    if ($status.protocolVersion -ne 1) { throw "unexpected protocol version $($status.protocolVersion)" }

    $listResult = Invoke-Ctl @('provider', 'list')
    if ($listResult.ExitCode -ne 0) { throw "provider list failed: $(ConvertTo-SafeBlocker $listResult.Text)" }
    $providers = @($listResult.Text | ConvertFrom-Json)
    $listed = @($providers | Where-Object { $_.provider -eq $Provider })
    if ($listed.Count -ne 1) { throw "provider list did not contain exactly one '$Provider' entry" }

    $probeResult = Invoke-Ctl @('provider', 'probe', $Provider)
    $probe = $null
    $handshakeStatus = 'FAIL'
    $handshakeBlocker = $null
    if ($probeResult.ExitCode -eq 0) {
        $probe = $probeResult.Text | ConvertFrom-Json
        switch ($probe.verification) {
            'configured' { $handshakeStatus = 'PASS' }
            'verified' { $handshakeStatus = 'PASS' }
            'unverified' { $handshakeStatus = 'PASS' }
            'not-detected' { $handshakeStatus = 'BLOCKED' }
            'degraded' { $handshakeStatus = 'BLOCKED' }
            default { $handshakeStatus = 'UNVERIFIED' }
        }
        $handshakeBlocker = ConvertTo-SafeBlocker ([string]$probe.degradationReason)
    } else {
        $handshakeStatus = 'BLOCKED'
        $handshakeBlocker = ConvertTo-SafeBlocker $probeResult.Text
    }

    $modelStatus = if ($HandshakeOnly) { 'UNVERIFIED' } else { 'BLOCKED' }
    $modelBlocker = if ($HandshakeOnly) { 'model-backed smoke was explicitly skipped' } else { 'readiness probe did not pass' }
    $outputMatched = $false
    $eventTypes = @()
    $usage = $null
    $knownAuthenticationMissing = $null -ne $probe -and
        $probe.authentication.native -in @('needs-login', 'blocked') -and
        $probe.authentication.external -ne 'ready'
    if (-not $HandshakeOnly -and $handshakeStatus -eq 'PASS' -and $knownAuthenticationMissing) {
        $modelBlocker = 'native authentication is required and no ready external credential was reported; no model prompt was sent'
    }
    if (-not $HandshakeOnly -and $handshakeStatus -eq 'PASS' -and -not $knownAuthenticationMissing) {
        $expectation = $expectations[$Provider]
        $smokeResult = Invoke-Ctl @('provider', 'smoke', $Provider, '--prompt', $expectation.Prompt)
        if ($smokeResult.ExitCode -ne 0) {
            $modelStatus = 'BLOCKED'
            $modelBlocker = ConvertTo-SafeBlocker $smokeResult.Text
        } else {
            $smoke = $smokeResult.Text | ConvertFrom-Json
            $events = @($smoke.events)
            $eventTypes = @($events | ForEach-Object { $_.type } | Select-Object -Unique)
            $usage = $events | Where-Object { $_.type -eq 'usage' } | Select-Object -Last 1
            $providerError = $events | Where-Object { $_.type -eq 'error' } | Select-Object -First 1
            $completed = $events | Where-Object { $_.type -eq 'turnCompleted' } | Select-Object -Last 1
            if ($null -ne $providerError) {
                $modelStatus = 'BLOCKED'
                $modelBlocker = ConvertTo-SafeBlocker ([string]$providerError.message)
            } elseif ($null -eq $completed) {
                $modelStatus = 'FAIL'
                $modelBlocker = 'smoke returned no terminal completion event'
            } else {
                $outputMatched = ([string]$completed.output).Trim() -eq $expectation.Expected
                if ($outputMatched) {
                    $modelStatus = 'PASS'
                    $modelBlocker = $null
                } else {
                    $modelStatus = 'FAIL'
                    $modelBlocker = 'terminal output did not match the exact conformance token'
                }
            }
        }
    }

    $candidate = $listed[0]
    $executablePath = if ($null -ne $probe -and $null -ne $probe.executable) { [string]$probe.executable } elseif ($null -ne $candidate.executable) { [string]$candidate.executable } else { '' }
    $capabilities = if ($null -ne $probe) { $probe.capabilities } else { $null }
    $authentication = if ($null -ne $probe) {
        [ordered]@{ native = $probe.authentication.native; external = $probe.authentication.external }
    } else { $null }
    $overallStatus = if ($handshakeStatus -eq 'PASS' -and $modelStatus -eq 'PASS') {
        'PASS'
    } elseif ($handshakeStatus -eq 'FAIL' -or $modelStatus -eq 'FAIL') {
        'FAIL'
    } elseif ($handshakeStatus -eq 'BLOCKED' -or $modelStatus -eq 'BLOCKED') {
        'BLOCKED'
    } else {
        'UNVERIFIED'
    }
    $entry = [ordered]@{
        provider = $Provider
        status = $overallStatus
        startedAt = $startedAt.ToString('o')
        completedAt = [DateTime]::UtcNow.ToString('o')
        durationMs = $timer.ElapsedMilliseconds
        binaries = [ordered]@{
            daemonSha256 = (Get-FileHash -LiteralPath $script:daemonExe -Algorithm SHA256).Hash
            cliSha256 = (Get-FileHash -LiteralPath $script:ctlExe -Algorithm SHA256).Hash
        }
        executable = [ordered]@{
            detected = [bool]$candidate.detected
            fileName = if ($executablePath -ne '') { [IO.Path]::GetFileName($executablePath) } else { $null }
            pathSha256 = if ($executablePath -ne '') { ConvertTo-PathHash $executablePath } else { $null }
            discoverySource = if ($null -ne $probe) { $probe.discoverySource } else { $candidate.discoverySource }
            version = if ($null -ne $probe) { $probe.version } else { $null }
        }
        handshake = [ordered]@{
            status = $handshakeStatus
            verification = if ($null -ne $probe) { $probe.verification } else { $null }
            protocol = if ($null -ne $probe) { $probe.protocol } else { $null }
            blocker = $handshakeBlocker
        }
        modelBacked = [ordered]@{
            status = $modelStatus
            outputMatched = $outputMatched
            eventTypes = $eventTypes
            usage = if ($null -ne $usage) {
                [ordered]@{
                    inputTokens = $usage.inputTokens
                    outputTokens = $usage.outputTokens
                    totalTokens = $usage.totalTokens
                    costUsd = $usage.costUsd
                }
            } else { $null }
            blocker = $modelBlocker
        }
        authentication = $authentication
        capabilities = $capabilities
        commands = @($script:commands)
    }
} catch {
    $entry = [ordered]@{
        provider = $Provider
        status = 'FAIL'
        startedAt = $startedAt.ToString('o')
        completedAt = [DateTime]::UtcNow.ToString('o')
        durationMs = $timer.ElapsedMilliseconds
        blocker = ConvertTo-SafeBlocker $_.Exception.Message
        commands = @($script:commands)
    }
    throw
} finally {
    $timer.Stop()
    if ($null -ne $daemon -and -not $daemon.HasExited) {
        Invoke-Ctl @('system', 'shutdown') | Out-Null
        if (-not $daemon.WaitForExit(5000)) { Stop-Process -Id $daemon.Id -Force }
    }
    $env:LAZOO_DATA_ROOT = $previousDataRoot

    if ($null -ne $entry) {
        $providerEntries = Read-ExistingProviders $manifestPath
        $providerEntries[$Provider] = $entry
        $manifestStatus = Get-CoreProviderStatus $providerEntries $requiredCoreProviders
        $manifest = [ordered]@{
            schemaVersion = 1
            status = $manifestStatus
            requiredProviders = $requiredCoreProviders
            excludedProviders = @([ordered]@{ provider = 'grok-build'; reason = 'Excluded by owner on 2026-10-16' })
            generatedAt = [DateTime]::UtcNow.ToString('o')
            platform = [Environment]::OSVersion.VersionString
            rustcVersion = (& rustc --version)
            cargoVersion = (& $cargoExe --version)
            providers = $providerEntries
        }
        $manifest | ConvertTo-Json -Depth 12 | Set-Content -LiteralPath $manifestPath -Encoding UTF8
    }
}

Write-Output "$($entry.status) provider=$Provider manifest=$manifestPath"
if ($HandshakeOnly) {
    if ($entry.handshake.status -ne 'PASS') { exit 1 }
} elseif ($entry.status -ne 'PASS') {
    exit 1
}
