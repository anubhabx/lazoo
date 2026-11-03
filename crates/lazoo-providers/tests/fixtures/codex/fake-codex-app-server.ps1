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
            if ($prompt -like '*VIEW_END_BURST*') {
                Set-Content -LiteralPath 'fake-codex-wait-view' -Value 'ready'
                # Bootstrap and history requests are served while canonical work is in flight.
                for ($readIndex = 0; $readIndex -lt 2; $readIndex++) {
                    $viewRead = [Console]::In.ReadLine() | ConvertFrom-Json
                    if ($viewRead.method -eq 'thread/read') {
                        Write-Message @{ id = $viewRead.id; result = @{ thread = @{ id = 'thread-1'; turns = @() } } }
                    } else {
                        Write-Message @{ id = $viewRead.id; result = @{ data = @(); nextCursor = $null } }
                    }
                }
                Write-Message @{ method = 'item/started'; params = @{ threadId = 'thread-1'; turnId = 'turn-1'; item = @{ id = 'cmd-1'; type = 'commandExecution'; status = 'inProgress'; command = 'cargo test' } } }
                Write-Message @{ method = 'item/commandExecution/outputDelta'; params = @{ threadId = 'thread-1'; turnId = 'turn-1'; itemId = 'cmd-1'; delta = 'tests passed' } }
                Write-Message @{ method = 'item/completed'; params = @{ threadId = 'thread-1'; turnId = 'turn-1'; item = @{ id = 'cmd-1'; type = 'commandExecution'; status = 'completed'; exitCode = 0 } } }
                # A final history refresh interleaves with a finite burst, then a delayed final.
                $viewRead = [Console]::In.ReadLine() | ConvertFrom-Json
                Write-Message @{ id = $viewRead.id; result = @{ data = @(); nextCursor = $null } }
                $frames = [System.Collections.Generic.List[string]]::new()
                for ($index = 0; $index -lt 64; $index++) {
                    $frames.Add((@{ method = 'item/agentMessage/delta'; params = @{ threadId = 'thread-1'; turnId = 'turn-1'; itemId = 'final-1'; delta = 'x' } } | ConvertTo-Json -Depth 10 -Compress))
                }
                $frames.Add((@{ method = 'thread/tokenUsage/updated'; params = @{ threadId = 'thread-1'; tokenUsage = @{ total = @{ inputTokens = 100; outputTokens = 64; totalTokens = 164 } } } } | ConvertTo-Json -Depth 10 -Compress))
                $frames.Add((@{ method = 'account/rateLimits/updated'; params = @{ rateLimits = @{ primary = @{ usedPercent = 25; windowDurationMins = 300 } } } } | ConvertTo-Json -Depth 10 -Compress))
                [Console]::Out.WriteLine(($frames -join "`n"))
                [Console]::Out.Flush()
                Start-Sleep -Milliseconds 200
                if (Test-Path -LiteralPath 'hold-final-for-reconnect') {
                    $deadline = [DateTime]::UtcNow.AddSeconds(5)
                    while (!(Test-Path -LiteralPath 'release-final') -and [DateTime]::UtcNow -lt $deadline) {
                        Start-Sleep -Milliseconds 10
                    }
                }
                Write-Message @{ method = 'item/completed'; params = @{ threadId = 'thread-1'; turnId = 'turn-1'; item = @{ id = 'final-1'; type = 'agentMessage'; text = ('x' * 64) } } }
                Write-Message @{ method = 'turn/completed'; params = @{ threadId = 'thread-1'; turn = @{ id = 'turn-1'; status = 'completed' } } }
                continue
            }
            if ($prompt -like '*EXIT_WITH_CODE*') { exit 37 }
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
