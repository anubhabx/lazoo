# Antigravity fixture that stalls mid-turn so cancellation can land. It never completes a turn.
param([Parameter(ValueFromRemainingArguments=$true)][string[]]$CliArgs)
[IO.File]::WriteAllText((Join-Path $PWD 'host-pid'), [string]$PID)
while (($line = [Console]::In.ReadLine()) -ne $null) {
  $message = $line | ConvertFrom-Json
  if ($message.event -ne 'user') { continue }
  [Console]::Out.WriteLine('{"event":"init","conversation_id":"conversation-1","init":{"cwd":"C:\repo","tools":["run_command"],"permission_mode":"request-review"}}')
  [Console]::Out.WriteLine('{"event":"step_update","step_update":{"conversation_id":"conversation-1","step_index":2,"state":"ACTIVE","step_type":"agent_response","text_delta":"before cancel"}}')
  [IO.File]::WriteAllText((Join-Path $PWD 'turn-started'), 'started')
  Start-Sleep -Seconds 30
  break
}
