"""Release-daemon parity acceptance: one bounded prompt, owned state, no Grok.

Run with claude-code, antigravity, or opencode. OpenCode uses only a local model
fixture. Other providers require explicit LAZOO_LIVE_PROVIDER_PARITY=1 consent.
Native screen captures and runtime state remain in ignored run-* directories.
"""
import asyncio
from datetime import datetime, timezone
import hashlib
from http.server import BaseHTTPRequestHandler, ThreadingHTTPServer
import json
import os
from pathlib import Path
import sqlite3
import sys
import threading
import uuid

BASE = Path(__file__).resolve().parent
REPO = BASE.parents[1]
sys.path.insert(0, str(BASE / ".deps"))
import pyte

PROVIDER = sys.argv[1]
assert PROVIDER in {"claude-code", "antigravity", "opencode"}
assert PROVIDER == "opencode" or os.environ.get("LAZOO_LIVE_PROVIDER_PARITY") == "1"
MARKER = "NATIVE_theater_7"
COMMAND_MARKER = "PROVIDER_PARITY_NOEXEC"


async def main():
    run = BASE / f"run-{PROVIDER}-{uuid.uuid4().hex}"
    run.mkdir()
    env = dict(os.environ, LAZOO_DATA_ROOT=str(run / "data"))
    env["LAZOO_NATIVE_HOOK_HELPER"] = str(REPO / "target/release/lazooctl.exe")
    evidence, errors, requests, commands = {}, [], [], []
    canonical_requests = []
    daemon = reader = submission = warm = terminal = session = fixture = None
    screen = bytearray()
    rendered = pyte.Screen(120, 40)
    stream = pyte.ByteStream(rendered)
    prompt_count = 0
    employee_name = "Parity lead " + uuid.uuid4().hex[:8]
    started = datetime.now(timezone.utc).isoformat()
    logs = [(run / name).open("wb") for name in ("daemon.stdout.log", "daemon.stderr.log")]

    class Fixture(BaseHTTPRequestHandler):
        def log_message(self, *_args):
            pass

        def do_POST(self):
            body = json.loads(self.rfile.read(int(self.headers["Content-Length"])))
            requests.append(body)
            has_roster = "lazoo_employees_list" in [item["function"]["name"] for item in body.get("tools", [])]
            if has_roster:
                canonical_requests.append(body)
            number = len(canonical_requests) if has_roster else 3
            tool = "lazoo_employees_list" if number == 1 else "bash"
            if number <= 2:
                assert tool in [item["function"]["name"] for item in body["tools"]]
                arguments = {} if number == 1 else {"command": f"echo {COMMAND_MARKER}", "description": "Bounded acceptance: decline this harmless command."}
                delta = {"tool_calls": [{"index": 0, "id": f"fixture-{number}", "type": "function", "function": {"name": tool, "arguments": json.dumps(arguments)}}]}
                if number == 1:
                    delta["content"] = MARKER
                reason = "tool_calls"
            else:
                delta, reason = {"content": MARKER}, "stop"
            chunks = [{"id": "fixture", "object": "chat.completion.chunk", "choices": [{"index": 0, "delta": value, "finish_reason": finish}]} for value, finish in [(delta, None), ({}, reason)]]
            payload = "".join("data: " + json.dumps(chunk) + "\n\n" for chunk in chunks) + "data: [DONE]\n\n"
            encoded = payload.encode()
            self.send_response(200)
            self.send_header("Content-Type", "text/event-stream")
            self.send_header("Content-Length", str(len(encoded)))
            self.end_headers()
            self.wfile.write(encoded)

    if PROVIDER == "opencode":
        fixture = ThreadingHTTPServer(("127.0.0.1", 0), Fixture)
        threading.Thread(target=fixture.serve_forever, daemon=True).start()
        config = run / "config/opencode"
        config.mkdir(parents=True)
        env["XDG_CONFIG_HOME"] = str(config.parent)
        (config / "opencode.json").write_text(json.dumps({"model": "fixture/echo", "small_model": "fixture/echo", "provider": {"fixture": {"npm": "@ai-sdk/openai-compatible", "name": "fixture", "options": {"baseURL": f"http://127.0.0.1:{fixture.server_port}/v1", "apiKey": "fixture"}, "models": {"echo": {"name": "echo"}}}}}), encoding="utf-8")

    async def ctl(*args, required=True):
        process = await asyncio.create_subprocess_exec(str(REPO / "target/release/lazooctl.exe"), "--json", *map(str, args), env=env, stdout=asyncio.subprocess.PIPE, stderr=asyncio.subprocess.PIPE)
        try:
            out, err = await asyncio.wait_for(process.communicate(), 100)
        except BaseException:
            if process.returncode is None:
                process.kill()
                await process.wait()
            raise
        commands.append({"args": list(map(str, args)), "exit": process.returncode})
        if process.returncode:
            if required:
                raise RuntimeError(f"ctl {args}: {err.decode(errors='replace')} {out.decode(errors='replace')}")
            return None
        return json.loads(out)

    async def read_screen():
        offset = 0
        while True:
            page = await ctl("terminal", "read", terminal["id"], "--expected-boot-id", terminal["bootId"], "--after", offset)
            chunk = bytes(page["bytes"])
            screen.extend(chunk)
            stream.feed(chunk)
            offset = page["output"]["end"]
            if not page["output"]["live"]:
                return
            await asyncio.sleep(.2)

    async def open_view(key):
        nonlocal reader, terminal, rendered, stream
        state = await ctl("session", "terminal-status", session["id"])
        assert state["viewable"], state
        opened = await ctl("session", "view", session["id"], "--expected-revision", state["sessionRevision"], "--columns", 120, "--rows", 40, "--idempotency-key", key)
        terminal = opened["terminal"]
        assert terminal["live"], terminal
        rendered = pyte.Screen(120, 40)
        stream = pyte.ByteStream(rendered)
        reader = asyncio.create_task(read_screen())

    async def close_view():
        nonlocal reader
        reader.cancel()
        await asyncio.gather(reader, return_exceptions=True)
        reader = None
        await ctl("terminal", "stop", terminal["id"], "--expected-boot-id", terminal["bootId"], "--expected-revision", terminal["revision"])

    async def wait_marker():
        for _ in range(100):
            if MARKER in "\n".join(rendered.display):
                return
            if reader.done():
                await reader
                break
            await asyncio.sleep(.1)
        raise AssertionError("actual native TUI did not render the retained assistant marker")

    try:
        daemon = await asyncio.create_subprocess_exec(str(REPO / "target/release/lazood.exe"), env=env, stdout=logs[0], stderr=logs[1])
        for _ in range(50):
            if await ctl("status", required=False):
                break
            await asyncio.sleep(.2)
        model = ("--model", "fixture/echo") if PROVIDER == "opencode" else ()
        if PROVIDER == "opencode":
            await ctl("provider", "smoke", "opencode", "--prompt", "Reply with READY. Do not use tools.")
            evidence["modelReadinessVerifiedWithLocalFixture"] = bool(requests)
        employee = (await ctl("employee", "create", employee_name, "--provider", PROVIDER, "--authority-mode", "ask", *model, "--idempotency-key", "employee"))["employee"]
        warm = asyncio.create_task(ctl("session", "warm", employee["id"], "--expected-employee-revision", employee["configurationRevision"], "--idempotency-key", "warm"))
        if PROVIDER != "opencode":
            trust = None
            for _ in range(400):
                pending = await ctl("native-trust", "list", "--state", "pending")
                if pending and pending[0]["notificationState"] == "settled":
                    trust = pending[0]
                    break
                if warm.done():
                    await warm
                    break
                await asyncio.sleep(.1)
            assert trust, "native startup bypassed the default trust ledger"
            evidence["trustConfirmationRequired"] = True
            evidence["trustNotificationSettled"] = trust["notificationState"] == "settled"
            await ctl("native-trust", "approve", trust["id"], "--expected-revision", trust["revision"], "--auto-accept-next-time", "--idempotency-key", "trust")
            policy = await ctl("native-trust", "policy", "show", PROVIDER)
            evidence["automaticNextTimeOption"] = policy["autoAcceptManagedNextTime"]
            await ctl("native-trust", "policy", "reset", PROVIDER, "--expected-revision", policy["revision"])
        session = (await warm)["session"]
        warm = None
        await open_view("first")
        if PROVIDER != "opencode":
            refused = await ctl("terminal", "write", terminal["id"], "--expected-boot-id", terminal["bootId"], "--text", "NEVER_TYPE", required=False)
            evidence["borrowedViewInputRefused"] = refused is None
        conversation = (await ctl("conversation", "create", employee["id"], "--title", "Provider parity acceptance", "--idempotency-key", "conversation"))["conversation"]
        command = f"Write-Output {COMMAND_MARKER}" if PROVIDER == "antigravity" else f"echo {COMMAND_MARKER}"
        prompt = f"Use the lazoo server's employees_list tool exactly once. Then attempt the native shell command exactly `{command}`. If refused, do not retry or use another tool. Finally reply only with the marker formed as NATIVE_, then the word theater, then an underscore and its letter count. Do no other work."
        prompt_count += 1
        submission = asyncio.create_task(ctl("turn", "submit", conversation["id"], "--text", prompt, "--authority-mode", "ask", "--idempotency-key", "turn"))
        declined, saw_command = set(), False
        for _ in range(150):
            pending = await ctl("approval", "list", "--conversation-id", conversation["id"], "--state", "pending")
            for approval in pending:
                if approval["id"] in declined:
                    continue
                detail = approval.get("detail", {})
                command_args = detail.get("toolCall", {}).get("args", detail.get("tool_input", detail.get("metadata", {})))
                if approval["action"].lower() in {"bash", "run_command"} and COMMAND_MARKER in json.dumps(command_args):
                    saw_command = True
                    await close_view()
                    current = await ctl("approval", "show", approval["id"])
                    evidence["viewClosePreservesApproval"] = current["state"] == "pending"
                    await open_view("during-approval-" + approval["id"])
                await ctl("approval", "decline", approval["id"], "--expected-revision", approval["revision"], "--idempotency-key", approval["id"])
                declined.add(approval["id"])
            if submission.done():
                break
            await asyncio.sleep(.5)
        submitted = await asyncio.wait_for(submission, 10)
        submission = None
        turn = await ctl("turn", "show", submitted["turn"]["id"])
        assert turn["state"] == "completed", turn
        evidence["durableCanonicalTurnCompleted"] = True
        evidence["nativeCommandReachedLazooAndWasDeclined"] = saw_command
        await wait_marker()
        evidence["nativeTuiRenderedCanonicalReply"] = True
        session = await ctl("session", "show", session["id"])
        assert not session["nativeSessionId"].startswith("pending:")
        native_id = session["nativeSessionId"]
        with sqlite3.connect(f"file:{(run / 'data/state/lazoo.db').as_posix()}?mode=ro", uri=True) as database:
            events = [json.loads(row[0]) for row in database.execute("SELECT payload_json FROM conversation_turn_events WHERE turn_id=? ORDER BY sequence", (turn["id"],))]
        (run / "turn-events.json").write_text(json.dumps(events, indent=2), encoding="utf-8")
        if PROVIDER == "opencode":
            # Native OpenCode ends its current generation when a permission is rejected.
            evidence["lazooRosterReturnedThroughMcp"] = len(canonical_requests) == 2 and employee_name in json.dumps(canonical_requests[1]) and "rosterRevision" in json.dumps(canonical_requests[1])
        else:
            evidence["nativeMcpCompletionObserved"] = any("completed" in json.dumps(event) and ("employees_list" in json.dumps(event) or "call_mcp_tool" in json.dumps(event)) for event in events)
        await close_view()
        evidence["viewClosePreservesSession"] = (await ctl("session", "show", session["id"]))["state"] == "idle"
        await ctl("session", "stop", session["id"], "--expected-revision", session["revision"], "--idempotency-key", "stop")
        session = (await ctl("session", "warm", employee["id"], "--expected-employee-revision", employee["configurationRevision"], "--idempotency-key", "resume"))["session"]
        evidence["daemonResumesSameNativeConversation"] = session["nativeSessionId"] == native_id
        screen.clear()
        await open_view("resumed")
        await wait_marker()
        evidence["resumedNativeHistory"] = True
        reader.cancel()
        await asyncio.gather(reader, return_exceptions=True)
        reader = None
        await ctl("session", "stop", session["id"], "--expected-revision", session["revision"], "--idempotency-key", "stop-resumed")
        closed = await ctl("terminal", "show", terminal["id"])
        evidence["sessionStopClosesNativeView"] = not closed["live"]
        assert all(evidence.values()), evidence
    except Exception as error:
        errors.append(repr(error))
    finally:
        for task in (reader, submission, warm):
            if task:
                task.cancel()
                await asyncio.gather(task, return_exceptions=True)
        if daemon and daemon.returncode is None:
            await ctl("system", "shutdown", required=False)
            try:
                await asyncio.wait_for(daemon.wait(), 15)
            except asyncio.TimeoutError:
                daemon.kill()
                await daemon.wait()
                errors.append("forced daemon cleanup required")
        for log in logs:
            log.close()
        if fixture:
            fixture.shutdown()
            fixture.server_close()
        (run / "screen.bin").write_bytes(screen)
        (run / "screen-rendered.json").write_text(json.dumps(rendered.display), encoding="utf-8")
        (run / "commands.json").write_text(json.dumps(commands, indent=2), encoding="utf-8")
        (run / "fixture-requests.json").write_text(json.dumps(requests, indent=2), encoding="utf-8")
        hashes = {name: hashlib.sha256((REPO / f"target/release/{name}.exe").read_bytes()).hexdigest() for name in ("lazood", "lazooctl", "lazoo-session-host")}
        result = {"status": "FAIL" if errors else "PASS", "provider": PROVIDER, "profile": "release", "run": str(run), "startedAt": started, "completedAt": datetime.now(timezone.utc).isoformat(), "evidence": evidence, "errors": errors, "canonicalPromptsSubmitted": prompt_count, "externalModelUse": prompt_count > 0 and PROVIDER != "opencode", "localFixtureRequests": len(requests), "binarySha256": hashes}
        (run / "result.json").write_text(json.dumps(result, indent=2), encoding="utf-8")
        print(json.dumps(result, indent=2))
    return bool(errors)


if __name__ == "__main__":
    sys.exit(asyncio.run(main()))
