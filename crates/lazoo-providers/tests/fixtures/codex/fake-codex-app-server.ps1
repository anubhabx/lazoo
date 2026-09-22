$ErrorActionPreference = 'Stop'

if ($args -contains '--version') {
    [Console]::Out.WriteLine('codex-cli 0.0.0-fixture')
    exit 0
}

function Write-Message([hashtable] $Message) {
    [Console]::Out.WriteLine(($Message | ConvertTo-Json -Depth 20 -Compress))
    [Console]::Out.Flush()
}

while ($null -ne ($line = [Console]::In.ReadLine())) {
    if ([string]::IsNullOrWhiteSpace($line)) {
        continue
    }
    $message = $line | ConvertFrom-Json
    switch ($message.method) {
        'initialize' {
            Write-Message @{ id = $message.id; result = @{ userAgent = 'fake-codex-app-server' } }
        }
        'initialized' {
            continue
        }
        'thread/start' {
            Write-Message @{ id = $message.id; result = @{ thread = @{ id = 'thread-1' } } }
        }
        'thread/resume' {
            Write-Message @{ id = $message.id; result = @{ thread = @{ id = 'thread-1' } } }
        }
        'turn/start' {
            $message.params | ConvertTo-Json -Depth 20 -Compress |
                Set-Content -LiteralPath (Join-Path (Get-Location) 'fake-codex-turn-start.json') -Encoding utf8
            Write-Message @{ id = $message.id; result = @{ turn = @{ id = 'turn-1' } } }
            Start-Sleep -Milliseconds 50
            Write-Message @{
                method = 'turn/started'
                params = @{ threadId = 'thread-1'; turn = @{ id = 'turn-1'; status = 'inProgress' } }
            }
            $prompt = ''
            if ($null -ne $message.params.input) {
                $prompt = [string]$message.params.input[0].text
            }
            if ($prompt -like '*REQUEST_APPROVAL*') {
                Write-Message @{
                    id = 'approval-1'
                    method = 'item/commandExecution/requestApproval'
                    params = @{
                        threadId = 'thread-1'
                        turnId = 'turn-1'
                        itemId = 'item-cmd-1'
                        command = 'cargo test'
                        cwd = 'C:\repo'
                        reason = 'run the test suite'
                    }
                }
                $approvalResponse = [Console]::In.ReadLine()
                $approvalResponse |
                    Set-Content -LiteralPath (Join-Path (Get-Location) 'fake-codex-approval-response.json') -Encoding utf8
            }
            Write-Message @{
                method = 'item/agentMessage/delta'
                params = @{ threadId = 'thread-1'; turnId = 'turn-1'; itemId = 'item-1'; delta = 'Hello' }
            }
            Write-Message @{
                method = 'turn/completed'
                params = @{ threadId = 'thread-1'; turn = @{ id = 'turn-1'; status = 'completed' } }
            }
        }
        'turn/interrupt' {
            $message.params | ConvertTo-Json -Depth 10 -Compress |
                Set-Content -LiteralPath (Join-Path (Get-Location) 'fake-codex-interrupt.json') -Encoding utf8
            Write-Message @{ id = $message.id; result = @{} }
        }
        default {
            if ($null -ne $message.id) {
                Write-Message @{ id = $message.id; error = @{ code = -32601; message = 'fixture method not found' } }
            }
        }
    }
}
