$ErrorActionPreference = 'Stop'

function Write-Message([hashtable] $Message) {
    [Console]::Out.WriteLine(($Message | ConvertTo-Json -Depth 20 -Compress))
    [Console]::Out.Flush()
}

function Save-Response([string] $Name) {
    while ($null -ne ($line = [Console]::In.ReadLine())) {
        $candidate = $line | ConvertFrom-Json
        if ($candidate.method -eq 'session/cancel') {
            'cancel-notification' | Set-Content -LiteralPath (Join-Path (Get-Location) 'fake-acp-cancel.txt') -Encoding utf8
            continue
        }
        if ($null -ne $candidate.id -and ($null -ne $candidate.result -or $null -ne $candidate.error)) {
            $candidate | ConvertTo-Json -Depth 20 -Compress | Set-Content -LiteralPath (Join-Path (Get-Location) $Name) -Encoding utf8
            return
        }
    }
}

while ($null -ne ($line = [Console]::In.ReadLine())) {
    if ([string]::IsNullOrWhiteSpace($line)) { continue }
    $message = $line | ConvertFrom-Json
    switch ($message.method) {
        'initialize' {
            Write-Message @{ jsonrpc = '2.0'; id = $message.id; result = @{
                protocolVersion = 1
                agentCapabilities = @{ loadSession = $false; sessionCapabilities = @{} }
            } }
        }
        'session/new' {
            Write-Message @{ jsonrpc = '2.0'; method = 'session/update'; params = @{ sessionId = 'session-1'; update = @{ sessionUpdate = 'agent_message_chunk'; content = @{ type = 'text'; text = 'stale-before-turn' } } } }
            Write-Message @{ jsonrpc = '2.0'; id = $message.id; result = @{ sessionId = 'session-1' } }
        }
        'session/prompt' {
            $prompt = [string]$message.params.prompt[0].text
            $session = if ($prompt -like '*MISMATCH*') { 'other-session' } else { 'session-1' }
            $kind = 'edit'
            foreach ($candidate in @('network', 'execute', 'delete', 'move', 'read', 'search', 'think', 'fetch', 'other')) {
                $marker = if ($candidate -eq 'network') { 'UNKNOWN_KIND' } else { 'KIND_' + $candidate.ToUpperInvariant() }
                if ($prompt -like "*$marker*") { $kind = $candidate; break }
            }
            if ($prompt -like '*EXECUTE*') { $kind = 'execute' }
            $options = if ($prompt -like '*MISSING_ONCE*') {
                @(@{ optionId = 'reject'; name = 'Reject'; kind = 'reject_once' })
            } else {
                @(@{ optionId = 'allow-one'; name = 'Allow once'; kind = 'allow_once' }, @{ optionId = 'allow-always'; name = 'Always'; kind = 'allow_always' })
            }
            $permissionId = if ($prompt -like '*NUMERIC_ID*') { 77 } else { 'permission-1' }
            $title = if ($prompt -like '*SECRET_TITLE*') { 'Authorization: Bearer secret-title-token' } else { 'Write fixture file' }
            Write-Message @{ jsonrpc = '2.0'; method = 'session/update'; params = @{ sessionId = 'session-1'; update = @{ sessionUpdate = 'agent_message_chunk'; content = @{ type = 'text'; text = 'before approval' } } } }
            'permission-emitted' | Set-Content -LiteralPath (Join-Path (Get-Location) 'fake-acp-pending.txt') -Encoding utf8
            Write-Message @{ jsonrpc = '2.0'; id = $permissionId; method = 'session/request_permission'; params = @{ sessionId = $session; toolCall = @{ toolCallId = 'call-1'; title = $title; kind = $kind; status = 'pending' }; options = $options } }
            Save-Response 'fake-acp-response-1.json'
            if ($prompt -like '*DUPLICATE*') {
                $secondId = if ($prompt -like '*NEW_ID*') { 'permission-2' } else { $permissionId }
                Write-Message @{ jsonrpc = '2.0'; id = $secondId; method = 'session/request_permission'; params = @{ sessionId = 'session-1'; toolCall = @{ toolCallId = 'call-1'; title = 'Write fixture file'; kind = 'edit'; status = 'pending' }; options = $options } }
                Save-Response 'fake-acp-response-2.json'
            }
            Write-Message @{ jsonrpc = '2.0'; id = $message.id; result = @{ stopReason = 'completed' } }
        }
        'session/close' { Write-Message @{ jsonrpc = '2.0'; id = $message.id; result = @{} } }
    }
}
