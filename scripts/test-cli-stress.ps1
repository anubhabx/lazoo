param(
    [int]$Invocations = 2048,
    [ValidateRange(1, 64)][int]$Concurrency = 16,
    [switch]$SkipBuild
)

$ErrorActionPreference = 'Stop'
Set-StrictMode -Version Latest

$repoRoot = (Resolve-Path (Join-Path $PSScriptRoot '..')).Path
$releaseDir = Join-Path $repoRoot 'target\release'
$daemonExe = Join-Path $releaseDir 'lazood.exe'
$ctlExe = Join-Path $releaseDir 'lazooctl.exe'
$runId = [Guid]::NewGuid().ToString('N')
$runRoot = Join-Path ([IO.Path]::GetTempPath()) "lazoo-cli-stress-$runId"
$dataRoot = Join-Path $runRoot 'data'
$artifactDir = Join-Path $repoRoot 'artifacts\acceptance'
$manifestPath = Join-Path $artifactDir "cli-stress-$runId.json"

function Start-Native([string]$FileName, [string[]]$Arguments) {
    $info = [Diagnostics.ProcessStartInfo]::new()
    $info.FileName = $FileName
    $info.UseShellExecute = $false
    $info.CreateNoWindow = $true
    $info.RedirectStandardOutput = $true
    $info.RedirectStandardError = $true
    $info.EnvironmentVariables['LAZOO_DATA_ROOT'] = $dataRoot
    # Arguments here are fixed literal read-only CLI commands; use the Windows PowerShell 5.1
    # compatible `Arguments` property rather than ProcessStartInfo.ArgumentList.
    $info.Arguments = $Arguments -join ' '
    $process = [Diagnostics.Process]::Start($info)
    [pscustomobject]@{ Process = $process; Started = [Diagnostics.Stopwatch]::StartNew(); Arguments = $Arguments; Stdout = $process.StandardOutput.ReadToEndAsync(); Stderr = $process.StandardError.ReadToEndAsync() }
}

function Complete-Native($Command) {
    if (-not $Command.Process.WaitForExit(30000)) {
        $script:timeoutDiagnostic = [ordered]@{
            pid = $Command.Process.Id
            arguments = $Command.Arguments
            elapsedMs = $Command.Started.ElapsedMilliseconds
            cpuMs = $Command.Process.TotalProcessorTime.TotalMilliseconds
            threadCount = $Command.Process.Threads.Count
            workingSetBytes = $Command.Process.WorkingSet64
        }
        $Command.Process.Kill()
        $null = $Command.Process.WaitForExit(5000)
        $script:timeoutDiagnostic.stdout = $Command.Stdout.GetAwaiter().GetResult()
        $script:timeoutDiagnostic.stderr = $Command.Stderr.GetAwaiter().GetResult()
        throw "CLI command timed out: $($Command.Arguments -join ' ')"
    }
    $Command.Started.Stop()
    [pscustomobject]@{
        exitCode = $Command.Process.ExitCode
        durationMs = $Command.Started.ElapsedMilliseconds
        stdout = $Command.Stdout.GetAwaiter().GetResult()
        stderr = $Command.Stderr.GetAwaiter().GetResult()
        arguments = $Command.Arguments
    }
}

New-Item -ItemType Directory -Force -Path $dataRoot, $artifactDir | Out-Null
$daemon = $null
$daemonStdout = ''
$daemonStderr = ''
$failure = ''
$results = [Collections.Generic.List[object]]::new()
$status = 'FAIL'
$binaryHashes = [ordered]@{}
$pending = [Collections.Generic.List[object]]::new()
$script:timeoutDiagnostic = $null
try {
    if (-not $SkipBuild) {
        & cargo build --workspace --release --locked
        if ($LASTEXITCODE -ne 0) { throw "release build exited $LASTEXITCODE" }
    }
    $binaryHashes.daemon = (Get-FileHash -LiteralPath $daemonExe -Algorithm SHA256).Hash
    $binaryHashes.cli = (Get-FileHash -LiteralPath $ctlExe -Algorithm SHA256).Hash
    $daemon = Start-Native $daemonExe @()
    $deadline = [DateTime]::UtcNow.AddSeconds(20)
    do {
        $probe = Complete-Native (Start-Native $ctlExe @('--json', 'status'))
        if ($probe.exitCode -eq 0) { break }
        Start-Sleep -Milliseconds 100
    } while ([DateTime]::UtcNow -lt $deadline)
    if ($probe.exitCode -ne 0) { throw "daemon readiness failed: $($probe.stderr)" }

    for ($index = 0; $index -lt $Invocations; $index++) {
        $arguments = if (($index % 2) -eq 0) { @('--json', 'status') } else { @('--json', 'system', 'resources') }
        $pending.Add((Start-Native $ctlExe $arguments))
        if ($pending.Count -eq $Concurrency -or $index -eq ($Invocations - 1)) {
            foreach ($command in $pending) { $results.Add((Complete-Native $command)) }
            $pending.Clear()
        }
    }
    $failures = @($results | Where-Object { $_.exitCode -ne 0 })
    $daemon.Process.Refresh()
    if ($failures.Count -ne 0) {
        # A client deadline names its phase on stderr (request write, response read, handshake).
        $deadlines = @($failures | Where-Object { $_.stderr -match 'timed out' })
        if ($deadlines.Count -ne 0) { throw "$($deadlines.Count) of $($failures.Count) failed CLI invocations hit a client deadline: $($deadlines[0].stderr.Trim())" }
        throw "$($failures.Count) CLI invocations failed"
    }
    if ($daemon.Process.HasExited) { throw "daemon exited during stress with $($daemon.Process.ExitCode)" }
    $status = 'PASS'
} catch {
    $failure = $_.Exception.Message
} finally {
    foreach ($command in $pending) {
        if (-not $command.Process.HasExited) { $command.Process.Kill(); $command.Process.WaitForExit() }
    }
    if ($null -ne $daemon) {
        if (-not $daemon.Process.HasExited) { $daemon.Process.Kill(); $daemon.Process.WaitForExit() }
        $daemonStdout = $daemon.Stdout.GetAwaiter().GetResult()
        $daemonStderr = $daemon.Stderr.GetAwaiter().GetResult()
    }
    $summary = [ordered]@{
        status = $status; runId = $runId; invocationsRequested = $Invocations; concurrency = $Concurrency
        binarySha256 = $binaryHashes
        successful = @($results | Where-Object { $_.exitCode -eq 0 }).Count
        failures = @($results | Where-Object { $_.exitCode -ne 0 } | Select-Object -First 50)
        exitCodes = @($results | Group-Object exitCode | ForEach-Object { [ordered]@{ code = [int]$_.Name; count = $_.Count } })
        failure = $failure; daemonStdout = $daemonStdout; daemonStderr = $daemonStderr
        timeoutDiagnostic = $script:timeoutDiagnostic
    }
    $summary | ConvertTo-Json -Depth 6 | Set-Content -LiteralPath $manifestPath -Encoding UTF8
    $resolvedRunRoot = (Resolve-Path -LiteralPath $runRoot -ErrorAction Stop).Path
    $expectedTempRoot = [IO.Path]::GetFullPath([IO.Path]::GetTempPath()).TrimEnd('\', '/')
    if ([IO.Path]::GetDirectoryName($resolvedRunRoot) -ne $expectedTempRoot -or
        [IO.Path]::GetFileName($resolvedRunRoot) -ne "lazoo-cli-stress-$runId") {
        throw "Refusing cleanup outside the exact stress-test directory: $resolvedRunRoot"
    }
    Remove-Item -LiteralPath $resolvedRunRoot -Recurse -Force -ErrorAction SilentlyContinue
}
if ($status -ne 'PASS') { throw "CLI stress failed; evidence: $manifestPath; $failure" }
Write-Output "CLI stress passed: $manifestPath"
