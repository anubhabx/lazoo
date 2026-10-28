$ErrorActionPreference = 'Stop'

while ($null -ne ($line = [Console]::In.ReadLine())) {
    if ([string]::IsNullOrWhiteSpace($line)) { continue }
    $message = $line | ConvertFrom-Json
    if ($message.method -eq 'initialize') {
        $response = @{ jsonrpc = '2.0'; id = $message.id; result = @{
            protocolVersion = 1
            agentCapabilities = @{ loadSession = $false; sessionCapabilities = @{} }
        } }
        [Console]::Out.WriteLine(($response | ConvertTo-Json -Depth 20 -Compress))
        [Console]::Out.Flush()
    }
}
