$ErrorActionPreference = 'Stop'

function Write-Message([hashtable] $Message) {
    [Console]::Out.WriteLine(($Message | ConvertTo-Json -Depth 12 -Compress))
    [Console]::Out.Flush()
}

while ($null -ne ($line = [Console]::In.ReadLine())) {
    if ([string]::IsNullOrWhiteSpace($line)) { continue }
    $message = $line | ConvertFrom-Json
    switch ($message.method) {
        'initialize' {
            $agentCapabilities = @{ loadSession = $true; sessionCapabilities = @{ close = $true }; mcpCapabilities = @{ http = $true } }
            if ($env:FAKE_ACP_NO_HTTP_MCP -eq '1') {
                $agentCapabilities.mcpCapabilities = @{}
            }
            Write-Message @{ jsonrpc = '2.0'; id = $message.id; result = @{
                protocolVersion = 1
                agentCapabilities = $agentCapabilities
            } }
        }
        'session/new' {
            $message.params | ConvertTo-Json -Depth 20 -Compress | Set-Content -LiteralPath (Join-Path (Get-Location) 'fake-acp-session-new.json') -Encoding utf8
            Write-Message @{ jsonrpc = '2.0'; id = $message.id; result = @{ sessionId = 'session-1' }
        } }
        'session/load' {
            $message.params | ConvertTo-Json -Depth 20 -Compress | Set-Content -LiteralPath (Join-Path (Get-Location) 'fake-acp-session-load.json') -Encoding utf8
            Write-Message @{ jsonrpc = '2.0'; id = $message.id; result = @{} }
        }
        'session/close' { Write-Message @{ jsonrpc = '2.0'; id = $message.id; result = @{} } }
    }
}
