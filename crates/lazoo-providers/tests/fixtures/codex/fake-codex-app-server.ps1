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
            $message.params | ConvertTo-Json -Depth 20 -Compress |
                Set-Content -LiteralPath (Join-Path (Get-Location) 'fake-codex-initialize.json') -Encoding utf8
            Write-Message @{ id = $message.id; result = @{ userAgent = 'fake-codex-app-server' } }
        }
        'initialized' {
            continue
        }
        'thread/start' {
            $message.params | ConvertTo-Json -Depth 20 -Compress |
                Set-Content -LiteralPath (Join-Path (Get-Location) 'fake-codex-thread-start.json') -Encoding utf8
            Write-Message @{ id = $message.id; result = @{ thread = @{ id = 'thread-1' } } }
        }
        'thread/resume' {
            Write-Message @{ id = $message.id; result = @{ thread = @{ id = 'thread-1' } } }
        }
        'thread/read' {
            Write-Message @{ id = $message.id; result = @{ thread = @{ id = 'thread-1'; cwd = (Get-Location).Path; canAcceptDirectInput = $true } } }
        }
        'thread/turns/list' {
            if (Test-Path -LiteralPath 'oversized-view-history') {
                Write-Message @{ id = $message.id; result = @{ data = @(@{ text = ('x' * (600 * 1024)) }); nextCursor = $null } }
                continue
            }
            if (Test-Path -LiteralPath 'flood-view-history') {
                for ($index = 0; $index -lt 128; $index++) {
                    Write-Message @{ method = 'thread/status/changed'; params = @{ threadId = 'thread-1' } }
                }
            }
            Write-Message @{ id = $message.id; error = @{ code = -32044; message = 'fixture history unavailable' } }
        }
        'thread/items/list' {
            Write-Message @{ id = $message.id; error = @{ code = -32044; message = 'fixture history unavailable' } }
        }
        'account/rateLimits/read' {
            Write-Message @{ id = $message.id; result = @{ rateLimits = @{ primary = @{ usedPercent = 25; windowDurationMins = 300; resetsAt = 2000000000 }; planType = 'pro' } } }
        }
        'account/read' {
            Write-Message @{ id = $message.id; result = @{ account = @{ id = 'fixture' } } }
        }
        'model/list' {
            Write-Message @{ id = $message.id; result = @{ data = @(); nextCursor = $null } }
        }
        'configRequirements/read' {
            if (@($message.params.PSObject.Properties).Count -ne 0) {
                Write-Message @{ id = $message.id; error = @{ code = -32602; message = 'configRequirements/read requires canonical empty params' } }
            } else {
                Write-Message @{ id = $message.id; result = @{} }
            }
        }
        'hooks/list' {
            if ($message.params.PSObject.Properties.Name -contains 'limit') {
                Write-Message @{ id = $message.id; error = @{ code = -32602; message = 'hooks/list rejects pagination fields' } }
            } else {
                Write-Message @{ id = $message.id; result = @{ data = @(); nextCursor = $null } }
            }
        }
        'skills/list' {
            if ($message.params.PSObject.Properties.Name -contains 'limit') {
                Write-Message @{ id = $message.id; error = @{ code = -32602; message = 'skills/list rejects pagination fields' } }
            } else {
                Write-Message @{ id = $message.id; result = @{ data = @(); nextCursor = $null } }
            }
        }
        'plugin/list' {
            if ($message.params.PSObject.Properties.Name -contains 'limit') {
                Write-Message @{ id = $message.id; error = @{ code = -32602; message = 'plugin/list rejects pagination fields' } }
            } else {
                Write-Message @{ id = $message.id; result = @{ data = @(); nextCursor = $null } }
            }
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
            if ($prompt -like '*VIEW_DURING_TURN*') {
                Set-Content -LiteralPath 'fake-codex-wait-view' -Value 'ready'
                $viewRead = [Console]::In.ReadLine() | ConvertFrom-Json
                Write-Message @{ id = $viewRead.id; result = @{ data = @(@{ text = ('x' * (600 * 1024)) }); nextCursor = $null } }
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
            if ($prompt -like '*CALL_TOOL*') {
                Write-Message @{
                    id = 'tool-1'
                    method = 'item/tool/call'
                    params = @{
                        threadId = 'thread-1'
                        turnId = 'turn-1'
                        callId = 'call-1'
                        tool = 'task_delegate'
                        arguments = @{
                            employeeId = 'employee-1'
                            objective = 'Fix the parser'
                            apiKey = 'sk-secret'
                        }
                    }
                }
                $toolResponse = [Console]::In.ReadLine()
                $toolResponse |
                    Set-Content -LiteralPath (Join-Path (Get-Location) 'fake-codex-tool-response.json') -Encoding utf8
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
