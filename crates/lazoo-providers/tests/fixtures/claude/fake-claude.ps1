param([Parameter(ValueFromRemainingArguments=$true)][string[]]$CliArgs)
if ($CliArgs -contains '--version') {
  [Console]::Out.WriteLine('2.1.296-fixture')
  exit 0
}
if ($CliArgs.Count -ge 3 -and $CliArgs[0] -eq 'auth' -and $CliArgs[1] -eq 'status' -and $CliArgs[2] -eq '--json') {
  switch ($env:FAKE_CLAUDE_AUTH_MODE) {
    'logged-out' {
      [Console]::Out.WriteLine('{"loggedIn":false,"authMethod":"none","apiProvider":"firstParty"}')
      exit 1
    }
    'malformed' {
      [Console]::Out.WriteLine('not-json')
      exit 1
    }
    'error' {
      [Console]::Out.WriteLine('{"loggedIn":false}')
      exit 2
    }
    default {
      [Console]::Out.WriteLine('{"loggedIn":true,"authMethod":"fixture","apiProvider":"firstParty"}')
      exit 0
    }
  }
}
$session = 'fixture-session'
for ($i = 0; $i -lt $CliArgs.Count - 1; $i++) { if ($CliArgs[$i] -eq '--session-id') { $session = $CliArgs[$i + 1] } }
$queued = $CliArgs -contains 'queued-startup'
$startupHooks = $CliArgs -contains 'startup-hooks'
$wrongSessionHook = $CliArgs -contains 'wrong-session-hook'
$nonStartupHook = $CliArgs -contains 'non-startup-hook'
$startupHookFlood = $CliArgs -contains 'startup-hook-flood'
if ($queued) {
  [Console]::Out.WriteLine((@{type='system'; subtype='init'; session_id=$session; resumed=$false} | ConvertTo-Json -Compress))
  [Console]::Out.WriteLine((@{type='result'; subtype='success'; session_id=$session; result='queued'} | ConvertTo-Json -Compress))
  [IO.File]::WriteAllText((Join-Path $PWD 'queued-ready'), 'ready')
}
if ($startupHooks -or $wrongSessionHook) {
  $hookSession = if ($wrongSessionHook) { 'wrong-session' } else { $session }
  [Console]::Out.WriteLine((@{type='system'; subtype='hook_started'; hook_id='startup-hook'; hook_name='SessionStart:startup'; hook_event='SessionStart'; session_id=$hookSession} | ConvertTo-Json -Compress))
  [Console]::Out.WriteLine((@{type='system'; subtype='hook_response'; hook_id='startup-hook'; hook_name='SessionStart:startup'; hook_event='SessionStart'; session_id=$hookSession; outcome='success'} | ConvertTo-Json -Compress))
  [IO.File]::WriteAllText((Join-Path $PWD 'startup-hooks-ready'), 'ready')
}
if ($nonStartupHook) {
  [Console]::Out.WriteLine((@{type='system'; subtype='hook_started'; hook_id='other-hook'; hook_name='SessionEnd:other'; hook_event='SessionEnd'; session_id=$session} | ConvertTo-Json -Compress))
  [IO.File]::WriteAllText((Join-Path $PWD 'startup-hooks-ready'), 'ready')
}
if ($startupHookFlood) {
  for ($i = 0; $i -le 64; $i++) {
    [Console]::Out.WriteLine((@{type='system'; subtype='hook_started'; hook_id=("startup-hook-" + $i); hook_name='SessionStart:startup'; hook_event='SessionStart'; session_id=$session} | ConvertTo-Json -Compress))
  }
  [IO.File]::WriteAllText((Join-Path $PWD 'startup-hooks-ready'), 'ready')
}
while (($line = [Console]::In.ReadLine()) -ne $null) {
  $message = $line | ConvertFrom-Json
  if ($message.type -ne 'user') { continue }
  if (-not $queued) {
    [Console]::Out.WriteLine((@{type='system'; subtype='init'; session_id=$session; resumed=$false} | ConvertTo-Json -Compress))
  }
  [Console]::Out.WriteLine((@{type='stream_event'; session_id=$session; event=@{type='content_block_delta'; delta=@{type='text_delta'; text='before permission'}}} | ConvertTo-Json -Compress -Depth 8))
  $request = @{type='control_request'; request_id='permission-1'; request=@{subtype='can_use_tool'; tool_name='Bash'; tool_use_id='toolu-fixture'; input=@{command='echo super-secret-token'}}}
  [Console]::Out.WriteLine(($request | ConvertTo-Json -Compress -Depth 8))
  $reply = ([Console]::In.ReadLine() | ConvertFrom-Json)
  $behavior = $reply.response.response.behavior
  if ($line -match 'duplicate') {
    [Console]::Out.WriteLine(($request | ConvertTo-Json -Compress -Depth 8))
    $duplicateReply = ([Console]::In.ReadLine() | ConvertFrom-Json)
    $behavior = $behavior + '/' + $duplicateReply.response.response.behavior
  }
  [Console]::Out.WriteLine((@{type='result'; subtype='success'; session_id=$session; result=$behavior; usage=@{input_tokens=1;output_tokens=1}} | ConvertTo-Json -Compress -Depth 8))
  break
}
