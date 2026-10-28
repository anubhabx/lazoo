"""Exercise the production daemon/native TUI against a local Responses simulator.

No external model, personal provider configuration, or user data is used. Requires
the installed native Codex CLI and built lazood/lazooctl/session-host binaries.
Run: python artifacts/readiness/native-tui/acceptance_production.py [debug|release]
Every run retains its disposable state and evidence together below this folder.
"""
import asyncio
from datetime import datetime, timezone
import hashlib
import json
import os
from pathlib import Path
import re
import subprocess
import sys
import threading
import time
import uuid
from http.server import BaseHTTPRequestHandler, ThreadingHTTPServer

BASE = Path(__file__).resolve().parent
REPO = BASE.parents[2]
PROFILE = sys.argv[1] if len(sys.argv) > 1 else "debug"
assert PROFILE in {"debug", "release"}


async def main():
    started_at = datetime.now(timezone.utc).isoformat()
    run = BASE / ("production-" + uuid.uuid4().hex)
    home = run / "codex-home"
    home.mkdir(parents=True)
    requests, commands, errors = [], [], []
    evidence = {}
    outputs = []
    terminal = daemon = session = None
    reader = submission = None
    output = bytearray()
    offset = 0

    class Fixture(BaseHTTPRequestHandler):
        def log_message(self, *_args):
            pass

        def do_POST(self):
            body = json.loads(self.rfile.read(int(self.headers.get("Content-Length", "0"))))
            requests.append(body)
            number = len(requests)
            response_id = f"local-fixture-{number}"
            events = [{"type": "response.created", "response": {"id": response_id}}]
            if number in (1, 3):
                name, arguments = ("employees_list", {}) if number == 1 else (
                    "exec_command", {"cmd": "echo NATIVE_APPROVAL_MUST_NOT_EXECUTE",
                                     "sandbox_permissions": "require_escalated",
                                     "justification": "Local acceptance: decline this simulated command."})
                events.append({"type": "response.output_item.done", "item": {
                    "type": "function_call", "call_id": f"fixture-call-{number}",
                    "name": name, "arguments": json.dumps(arguments)}})
            else:
                text = "NATIVE_VIEW_CONTROLLER_STREAM_OK" if number == 2 else "NATIVE_VIEW_APPROVAL_DECLINED_OK"
                item = {"type": "message", "role": "assistant", "id": f"fixture-message-{number}",
                        "content": [{"type": "output_text", "text": text}]}
                events.extend([
                    {"type": "response.output_item.added", "item": dict(item, content=[])},
                    {"type": "response.output_text.delta", "delta": text},
                    {"type": "response.output_item.done", "item": item},
                ])
            events.append({"type": "response.completed", "response": {
                "id": response_id, "usage": {"input_tokens": 0, "output_tokens": 0, "total_tokens": 0}}})
            self.send_response(200)
            self.send_header("Content-Type", "text/event-stream")
            self.send_header("Connection", "close")
            self.end_headers()
            for event in events:
                self.wfile.write(("event: " + event["type"] + "\ndata: " + json.dumps(event) + "\n\n").encode())
                self.wfile.flush()
                time.sleep(.35)

    fixture = ThreadingHTTPServer(("127.0.0.1", 0), Fixture)
    server_thread = threading.Thread(target=fixture.serve_forever, daemon=True)
    server_thread.start()
    (home / "config.toml").write_text(
        'model = "probe-model"\nmodel_provider = "probe"\n'
        '[model_providers.probe]\nname = "Local acceptance simulator"\n'
        f'base_url = "http://127.0.0.1:{fixture.server_port}/v1"\n'
        'wire_api = "responses"\nrequires_openai_auth = false\nsupports_websockets = false\n', encoding="utf-8")
    env = dict(os.environ, CODEX_HOME=str(home), LAZOO_DATA_ROOT=str(run / "data"))
    for name in ("CODEX_THREAD_ID", "CODEX_SESSION_ID", "OPENAI_API_KEY", "CODEX_API_KEY"):
        env.pop(name, None)
    if os.environ.get("LAZOO_PROBE_CODEX"):
        env["PATH"] = str(Path(os.environ["LAZOO_PROBE_CODEX"]).parent) + os.pathsep + env["PATH"]
    log_out = (run / "daemon-stdout.log").open("wb")
    log_err = (run / "daemon-stderr.log").open("wb")

    async def ctl(*args, required=True):
        process = await asyncio.create_subprocess_exec(
            str(REPO / f"target/{PROFILE}/lazooctl.exe"), "--json", *map(str, args),
            env=env, stdout=asyncio.subprocess.PIPE, stderr=asyncio.subprocess.PIPE)
        try:
            out, err = await asyncio.wait_for(process.communicate(), 45)
        except BaseException:
            process.kill()
            await process.wait()
            raise
        commands.append({"args": list(map(str, args)), "exit": process.returncode})
        if process.returncode:
            if required:
                raise RuntimeError(f"ctl {args}: {err.decode(errors='replace')} {out.decode(errors='replace')}")
            return None
        value = json.loads(out)
        outputs.append(value)
        return value

    async def read_screen():
        nonlocal offset
        while True:
            page = await ctl("terminal", "read", terminal["id"], "--expected-boot-id", terminal["bootId"], "--after", offset)
            chunk = bytes(page["bytes"])
            output.extend(chunk)
            offset = page["output"]["end"]
            for query, answer in [(b"\x1b[6n", "\x1b[1;1R"), (b"\x1b[c", "\x1b[?1;2c"), (b"\x1b[>c", "\x1b[>0;1;0c")]:
                if query in chunk:
                    await ctl("terminal", "write", terminal["id"], "--expected-boot-id", terminal["bootId"], "--text", answer)
            if not page["output"]["live"]:
                return
            await asyncio.sleep(.2)

    async def open_view(key):
        nonlocal terminal, reader, offset, output
        state = await ctl("session", "terminal-status", session["id"])
        assert state["viewable"], state
        opened = await ctl("session", "view", session["id"], "--expected-revision", state["sessionRevision"],
                           "--columns", 110, "--rows", 36, "--idempotency-key", key)
        terminal = opened["terminal"]
        assert terminal["live"] and "--remote" in terminal["args"], terminal
        assert terminal["args"][1] == session["nativeSessionId"], terminal
        offset, output = 0, bytearray()
        reader = asyncio.create_task(read_screen())
        return opened

    async def wait_screen(needle, seconds=30):
        for _ in range(seconds * 5):
            if needle.encode() in output:
                return
            if reader.done():
                await reader
                raise RuntimeError("native TUI exited before screen marker: " + needle)
            await asyncio.sleep(.2)
        raise RuntimeError("native TUI missing screen marker: " + needle)

    async def stop_view():
        nonlocal reader
        reader.cancel()
        await asyncio.gather(reader, return_exceptions=True)
        reader = None
        await ctl("terminal", "stop", terminal["id"], "--expected-boot-id", terminal["bootId"],
                  "--expected-revision", terminal["revision"])

    async def completed(turn_id, seconds=45):
        for _ in range(seconds * 2):
            turn = await ctl("turn", "show", turn_id)
            if turn["state"] == "completed":
                return turn
            assert turn["state"] in {"recorded", "submitting", "accepted"}, turn
            await asyncio.sleep(.5)
        raise RuntimeError("canonical turn did not complete")

    try:
        daemon = await asyncio.create_subprocess_exec(str(REPO / f"target/{PROFILE}/lazood.exe"), env=env,
                                                       stdout=log_out, stderr=log_err)
        for _ in range(40):
            if await ctl("status", required=False):
                break
            await asyncio.sleep(.2)
        employee = (await ctl("employee", "create", "Native acceptance lead", "--provider", "codex",
                              "--model", "probe-model", "--authority-mode", "ask", "--idempotency-key", "employee"))["employee"]
        session = (await ctl("session", "warm", employee["id"], "--expected-employee-revision", employee["configurationRevision"],
                             "--idempotency-key", "warm"))["session"]
        (run / "session.json").write_text(json.dumps(session, indent=2), encoding="utf-8")
        first = await open_view("view-first")
        same = await ctl("session", "view", session["id"], "--expected-revision", session["revision"],
                         "--columns", 110, "--rows", 36, "--idempotency-key", "view-first")
        evidence["idempotentView"] = first == same
        await wait_screen("direct input is disabled")
        evidence["genuineReadOnlyNativeTui"] = True
        conversation = (await ctl("conversation", "create", employee["id"], "--title", "Native view acceptance",
                                  "--idempotency-key", "conversation"))["conversation"]
        submitted = await ctl("turn", "submit", conversation["id"], "--text", "Use employees_list for the local test.",
                              "--authority-mode", "ask", "--idempotency-key", "turn-first")
        first_turn = await completed(submitted["turn"]["id"])
        (run / "turn-first.json").write_text(json.dumps(first_turn, indent=2), encoding="utf-8")
        await wait_screen("NATIVE_VIEW_CONTROLLER_STREAM_OK")
        evidence["controllerStreamInNativeTui"] = True
        evidence["collaborationToolAnsweredByLazoo"] = "Native acceptance lead" in json.dumps(requests[1])
        (run / "screen-first.bin").write_bytes(output)
        await stop_view()
        shown = await ctl("session", "show", session["id"])
        evidence["viewClosePreservesSession"] = shown["state"] == "idle" and shown["nativeSessionId"] == session["nativeSessionId"]
        await open_view("view-reconnect")
        await wait_screen("NATIVE_VIEW_CONTROLLER_STREAM_OK")
        evidence["nativeHistoryOnReconnect"] = True
        submission = asyncio.create_task(ctl(
            "turn", "submit", conversation["id"], "--text", "Exercise the local approval fixture.",
            "--authority-mode", "ask", "--idempotency-key", "turn-second"))
        approval = None
        for _ in range(60):
            pending = await ctl("approval", "list", "--conversation-id", conversation["id"], "--state", "pending")
            if pending:
                approval = pending[0]
                break
            await asyncio.sleep(.5)
        assert approval is not None, "native execution request never reached the Lazoo approval ledger"
        await stop_view()
        pending_turn = await ctl("turn", "show", approval["turnId"])
        still_pending = await ctl("approval", "show", approval["id"])
        evidence["closingViewPreservesPendingTurnAndApproval"] = (
            pending_turn["state"] == "accepted" and still_pending["state"] == "pending")
        await open_view("view-during-approval")
        await wait_screen("direct input is disabled")
        decision = await ctl("approval", "decline", approval["id"], "--expected-revision", approval["revision"],
                             "--idempotency-key", "decline-fixture")
        evidence["approvalDecidedByLazoo"] = decision["approval"]["state"] == "declined"
        submitted = await submission
        second_turn = await completed(submitted["turn"]["id"])
        (run / "turn-second.json").write_text(json.dumps(second_turn, indent=2), encoding="utf-8")
        await wait_screen("NATIVE_VIEW_APPROVAL_DECLINED_OK")
        evidence["nativeStreamAfterApproval"] = True
        evidence["nativeCommandWasDeclined"] = any(
            item.get("type") == "function_call_output"
            and item.get("call_id") == "fixture-call-3"
            and "rejected by user" in item.get("output", "")
            for item in requests[-1].get("input", []))
        state = await ctl("session", "terminal-status", session["id"])
        evidence["statusRecoversView"] = state["viewTerminal"]["id"] == terminal["id"]
        shown = await ctl("session", "show", session["id"])
        await ctl("session", "stop", session["id"], "--expected-revision", shown["revision"], "--idempotency-key", "stop-session")
        await asyncio.wait_for(reader, 8)
        reader = None
        evidence["sessionStopClosesNativeTui"] = True
        evidence["fourLocalModelRequests"] = len(requests) == 4
        assert all(evidence.values()), evidence
    except Exception as error:
        errors.append(repr(error))
    finally:
        if submission:
            submission.cancel()
            await asyncio.gather(submission, return_exceptions=True)
        if reader:
            reader.cancel()
            await asyncio.gather(reader, return_exceptions=True)
        if daemon and daemon.returncode is None:
            await ctl("system", "shutdown", required=False)
            try:
                await asyncio.wait_for(daemon.wait(), 15)
            except asyncio.TimeoutError:
                daemon.kill()
                await daemon.wait()
                errors.append("daemon needed forced cleanup")
        log_out.close()
        log_err.close()
        fixture.shutdown()
        fixture.server_close()
        server_thread.join(timeout=2)
        (run / "screen-final.bin").write_bytes(output)
        (run / "screen-final.txt").write_text(re.sub(r"\x1b\[[0-?]*[ -/]*[@-~]", "", output.decode(errors="replace")), encoding="utf-8")
        for name, value in [("fixture-requests", requests), ("commands", commands), ("rpc-results", outputs)]:
            (run / (name + ".json")).write_text(json.dumps(value, indent=2), encoding="utf-8")
        def sha256(path):
            digest = hashlib.sha256()
            with Path(path).open("rb") as source:
                for block in iter(lambda: source.read(1024 * 1024), b""):
                    digest.update(block)
            return digest.hexdigest()

        plain_screen = re.sub(r"\x1b\[[0-?]*[ -/]*[@-~]", "", output.decode(errors="replace"))
        native_version = re.search(r"OpenAI Codex \(v([^\)]+)\)", plain_screen)
        source_revision = subprocess.run(
            ["git", "rev-parse", "HEAD"], cwd=REPO, capture_output=True, text=True, check=True).stdout.strip()
        binaries = {name: sha256(REPO / f"target/{PROFILE}/{name}.exe")
                    for name in ("lazood", "lazooctl", "lazoo-session-host")}
        if terminal:
            binaries["nativeCodex"] = sha256(terminal["executable"])
        result = {"status": "PASS" if not errors else "FAIL", "run": str(run), "profile": PROFILE,
                  "startedAt": started_at, "completedAt": datetime.now(timezone.utc).isoformat(),
                  "sourceRevision": source_revision, "driverSha256": sha256(__file__),
                  "binarySha256": binaries, "nativeExecutable": terminal["executable"] if terminal else None,
                  "nativeVersion": native_version.group(1) if native_version else None,
                  "errors": errors, "evidence": evidence, "externalModelCalls": 0,
                  "localFixtureRequests": len(requests)}
        (run / "result.json").write_text(json.dumps(result, indent=2), encoding="utf-8")
        print(json.dumps(result, indent=2))
    return 1 if errors else 0


if __name__ == "__main__":
    sys.exit(asyncio.run(main()))
