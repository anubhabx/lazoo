"""Bounded native-TUI startup probe, not production code. Sends no model turns.

Dependencies: python -m pip install --target .deps websockets==17.3
All processes, provider state, logs and terminal output belong to one disposable run.
The native viewer gets only a scoped read interface over the controller's stdio host.
"""
import asyncio
import json
import os
from pathlib import Path
import re
import secrets
import sys
import threading
import time
import uuid
from http.server import BaseHTTPRequestHandler, ThreadingHTTPServer

BASE = Path(__file__).resolve().parent
sys.path.insert(0, str(BASE / ".deps"))
from websockets.asyncio.server import serve

REPO = BASE.parents[2]
CODEX = Path(r"C:\nvm4w\nodejs\node_modules\@openai\codex\node_modules\@openai\codex-win32-x64\vendor\x86_64-pc-windows-msvc\bin\codex.exe")


async def main():
    run = BASE / ("run-" + uuid.uuid4().hex)
    run.mkdir()
    home = run / "codex-home"
    home.mkdir()
    fixture_requests = []
    class Fixture(BaseHTTPRequestHandler):
        def log_message(self, *_args):
            pass
        def do_POST(self):
            body = json.loads(self.rfile.read(int(self.headers.get("Content-Length", "0"))))
            fixture_requests.append(body)
            number = len(fixture_requests)
            response_id = f"fixture-response-{number}"
            events = [{"type": "response.created", "response": {"id": response_id}}]
            if number == 1:
                events.append({"type": "response.output_item.done", "item": {"type": "function_call", "call_id": "native-view-call", "name": "demo_tool", "arguments": '{"city":"Paris"}'}})
            else:
                message = {"type": "message", "role": "assistant", "id": "native-view-message", "content": [{"type": "output_text", "text": "NATIVE_VIEW_CONTROLLER_STREAM_OK"}]}
                events.extend([
                    {"type": "response.output_item.added", "item": dict(message, content=[{"type": "output_text", "text": ""}])},
                    {"type": "response.output_text.delta", "delta": "NATIVE_VIEW_"},
                    {"type": "response.output_text.delta", "delta": "CONTROLLER_STREAM_OK"},
                    {"type": "response.output_item.done", "item": message},
                ])
            events.append({"type": "response.completed", "response": {"id": response_id, "usage": {"input_tokens": 0, "output_tokens": 0, "total_tokens": 0}}})
            self.send_response(200)
            self.send_header("Content-Type", "text/event-stream")
            self.send_header("Connection", "close")
            self.end_headers()
            for event in events:
                self.wfile.write(("event: " + event["type"] + "\ndata: " + json.dumps(event) + "\n\n").encode())
                self.wfile.flush()
                time.sleep(.3)
    fixture = ThreadingHTTPServer(("127.0.0.1", 0), Fixture)
    fixture_thread = threading.Thread(target=fixture.serve_forever, daemon=True)
    fixture_thread.start()
    (home / "config.toml").write_text('model = "probe-model"\nmodel_provider = "probe"\n[model_providers.probe]\nname = "Local protocol probe"\nbase_url = "http://127.0.0.1:' + str(fixture.server_port) + '/v1"\nwire_api = "responses"\nrequires_openai_auth = false\nsupports_websockets = false\n', encoding="utf-8")
    cwd = run / "work"
    cwd.mkdir()
    env = dict(os.environ, CODEX_HOME=str(home), LAZOO_DATA_ROOT=str(run / "data"), TERM="xterm-256color", COLORTERM="truecolor",
               LAZOO_NATIVE_TUI_PROBE_TOKEN=secrets.token_urlsafe(32))
    pending = {}
    viewers = set()
    trace = []
    next_id = 0
    terminal = None
    upstream = None
    daemon = None
    reader = None
    output = bytearray()
    errors = []
    tool_calls = []
    native_events = []
    native_turns = 0
    upstream_error = (run / "upstream-stderr.log").open("wb")
    daemon_error = (run / "daemon-stderr.log").open("wb")
    daemon_output = (run / "daemon-stdout.log").open("wb")

    async def ctl(*args, required=True):
        proc = await asyncio.create_subprocess_exec(
            str(REPO / "target/debug/lazooctl.exe"), "--json", *map(str, args),
            env=env, stdout=asyncio.subprocess.PIPE, stderr=asyncio.subprocess.PIPE)
        try:
            out, err = await asyncio.wait_for(proc.communicate(), 25)
        except BaseException:
            proc.kill()
            await proc.wait()
            raise
        if proc.returncode:
            if required:
                raise RuntimeError(f"ctl {args[0]}: {err.decode(errors='replace')}")
            return None
        return json.loads(out)

    async def read_upstream():
        while line := await upstream.stdout.readline():
            frame = json.loads(line)
            if "id" in frame and "method" not in frame:
                future = pending.pop(str(frame["id"]), None)
                if future and not future.done():
                    future.set_result(frame)
            elif "method" in frame and "id" not in frame:
                native_events.append(frame["method"])
                for viewer in list(viewers):
                    try:
                        await asyncio.wait_for(viewer.send(json.dumps(frame)), 2)
                    except Exception:
                        viewers.discard(viewer)
            elif "method" in frame:
                if frame["method"] in {"item/tool/call", "item/dynamicToolCall"} and frame.get("params", {}).get("tool") == "demo_tool":
                    tool_calls.append(frame["params"])
                    upstream.stdin.write((json.dumps({"id": frame["id"], "result": {"contentItems": [{"type": "inputText", "text": "dynamic-ok"}], "success": True}}) + "\n").encode())
                    await upstream.stdin.drain()
                else:
                    trace.append({"unexpectedServerRequest": frame["method"]})
        for future in pending.values():
            if not future.done():
                future.set_exception(RuntimeError("upstream closed"))

    async def rpc(method, params):
        nonlocal next_id
        next_id += 1
        request_id = f"controller-{next_id}"
        future = asyncio.get_running_loop().create_future()
        pending[request_id] = future
        upstream.stdin.write((json.dumps({"id": request_id, "method": method, "params": params}) + "\n").encode())
        await upstream.stdin.drain()
        frame = await asyncio.wait_for(future, 15)
        if "error" in frame:
            raise RuntimeError(json.dumps(frame["error"]))
        return frame["result"]

    try:
        upstream = await asyncio.create_subprocess_exec(
            str(CODEX), "app-server", "--listen", "stdio://", cwd=cwd, env=env,
            stdout=asyncio.subprocess.PIPE, stdin=asyncio.subprocess.PIPE,
            stderr=upstream_error, limit=4 * 1024 * 1024)
        reader = asyncio.create_task(read_upstream())
        initialized = await rpc("initialize", {"clientInfo": {"name": "lazoo_native_probe", "version": "0.1.0"}, "capabilities": {"experimentalApi": True}})
        upstream.stdin.write(b'{"method":"initialized"}\n')
        await upstream.stdin.drain()
        started = await rpc("thread/start", {"cwd": str(cwd), "approvalPolicy": "never", "sandbox": "read-only", "ephemeral": False, "dynamicTools": [{"name": "demo_tool", "description": "Local fixture tool", "inputSchema": {"type": "object", "properties": {"city": {"type": "string"}}, "required": ["city"], "additionalProperties": False}}]})
        thread_id = started["thread"]["id"]
        (run / "thread-start.json").write_text(json.dumps(started, indent=2), encoding="utf-8")

        async def viewer(connection):
            if connection.request.headers.get("Authorization") != "Bearer " + env["LAZOO_NATIVE_TUI_PROBE_TOKEN"]:
                await connection.close(code=1008, reason="authentication required")
                return
            viewers.add(connection)
            try:
                async for raw in connection:
                    frame = json.loads(raw)
                    method = frame.get("method")
                    params = frame.get("params") or {}
                    trace.append({"method": method, "params": params, "hasId": "id" in frame})
                    if "id" not in frame:
                        continue
                    try:
                        if method == "initialize":
                            result = initialized
                        elif method == "thread/resume" and params.get("threadId") == thread_id:
                            # Bootstrap the genuine native UI from the controller's active thread/config.
                            read = await rpc("thread/read", {"threadId": thread_id, "includeTurns": False})
                            result = dict(started, thread=read["thread"])
                            result["thread"]["canAcceptDirectInput"] = False
                            trace.append({"rejoinPolicy": result.get("approvalPolicy"), "sameThread": result.get("thread", {}).get("id") == thread_id})
                        elif method in {"model/list", "account/read", "account/rateLimits/read", "config/read", "configRequirements/read", "skills/list", "hooks/list", "plugin/list", "mcpServerStatus/list", "app/list", "collaborationMode/list", "experimentalFeature/list"}:
                            if method == "account/read":
                                params["refreshToken"] = False
                            result = await rpc(method, params)
                        elif method in {"thread/turns/list", "thread/items/list"} and params.get("threadId") == thread_id:
                            # Codex cannot page a newly created unmaterialized empty thread.
                            if native_turns == 0:
                                result = {"data": [], "nextCursor": None}
                            else:
                                result = await rpc(method, params)
                        elif method == "thread/read" and params.get("threadId") == thread_id:
                            result = await rpc(method, params)
                        elif method == "thread/list":
                            result = {"data": [started["thread"]], "nextCursor": None}
                        elif method == "thread/unsubscribe":
                            result = {}
                        else:
                            raise RuntimeError("viewer method is not allowed: " + str(method))
                        reply = {"id": frame["id"], "result": result}
                    except Exception as error:
                        trace.append({"methodError": method, "error": str(error)})
                        reply = {"id": frame["id"], "error": {"code": -32001, "message": str(error)}}
                    await connection.send(json.dumps(reply))
            except Exception as error:
                trace.append({"viewerClosed": type(error).__name__})
            finally:
                viewers.discard(connection)

        async with serve(viewer, "127.0.0.1", 0, origins=[None], compression=None, max_size=1024 * 1024, close_timeout=2) as server:
            port = server.sockets[0].getsockname()[1]
            daemon = await asyncio.create_subprocess_exec(str(REPO / "target/debug/lazood.exe"), env=env, stdout=daemon_output, stderr=daemon_error)
            for _ in range(40):
                if await ctl("status", required=False):
                    break
                await asyncio.sleep(.15)
            arguments = ["resume", thread_id, "--remote", f"ws://127.0.0.1:{port}", "--remote-auth-token-env", "LAZOO_NATIVE_TUI_PROBE_TOKEN", "--no-alt-screen"]
            open_args = ["terminal", "open", str(CODEX), "--cwd", str(cwd), "--columns", "100", "--rows", "32"]
            open_args.extend("--arg=" + argument for argument in arguments)
            terminal = await ctl(*open_args)
            (run / "terminal.json").write_text(json.dumps(terminal, indent=2), encoding="utf-8")
            offset = 0
            for _ in range(24):
                await asyncio.sleep(.5)
                page = await ctl("terminal", "read", terminal["id"], "--expected-boot-id", terminal["bootId"], "--after", offset)
                chunk = bytes(page["bytes"])
                offset = page["output"]["end"]
                output.extend(chunk)
                for query, answer in [(b"\x1b[6n", "\x1b[1;1R"), (b"\x1b[c", "\x1b[?1;2c"), (b"\x1b[>c", "\x1b[>0;1;0c")]:
                    if query in chunk:
                        await ctl("terminal", "write", terminal["id"], "--expected-boot-id", terminal["bootId"], "--text", answer)
                if not page["output"]["live"]:
                    break
                if native_turns == 0 and any(item.get("method") == "thread/items/list" for item in trace):
                    await rpc("turn/start", {"threadId": thread_id, "input": [{"type": "text", "text": "Run the local fixture tool and print its simulated result."}]})
                    native_turns += 1
            for method, params in [("thread/read", {"threadId": thread_id, "includeTurns": True}), ("thread/turns/list", {"threadId": thread_id, "limit": 5}), ("thread/items/list", {"threadId": thread_id, "limit": 100})]:
                try:
                    history = await rpc(method, params)
                    trace.append({"historyProbe": method, "keys": list(history), "rows": len(history.get("data", history.get("thread", {}).get("turns", [])))})
                except Exception as error:
                    trace.append({"historyProbe": method, "error": str(error)})
    except Exception as error:
        errors.append(repr(error))
    finally:
        if terminal:
            try:
                await ctl("terminal", "stop", terminal["id"], "--expected-boot-id", terminal["bootId"], "--expected-revision", terminal["revision"])
            except Exception as error:
                errors.append("terminal cleanup: " + repr(error))
        if daemon and daemon.returncode is None:
            await ctl("shutdown", required=False)
            try:
                await asyncio.wait_for(daemon.wait(), 5)
            except asyncio.TimeoutError:
                daemon.kill()
                await daemon.wait()
        if upstream and upstream.returncode is None:
            upstream.stdin.close()
            try:
                await asyncio.wait_for(upstream.wait(), 3)
            except asyncio.TimeoutError:
                upstream.kill()
                await upstream.wait()
        if reader:
            reader.cancel()
            await asyncio.gather(reader, return_exceptions=True)
        await asyncio.sleep(.1)
        for handle in (upstream_error, daemon_error, daemon_output):
            handle.close()
        fixture.shutdown()
        fixture.server_close()
        fixture_thread.join(timeout=2)
        (run / "requests.json").write_text(json.dumps(trace, indent=2), encoding="utf-8")
        (run / "screen-raw.bin").write_bytes(output)
        plain = re.sub(r"\x1b\[[0-?]*[ -/]*[@-~]", "", output.decode(errors="replace"))
        (run / "screen.txt").write_text(plain, encoding="utf-8")
        (run / "fixture-requests.json").write_text(json.dumps(fixture_requests, indent=2), encoding="utf-8")
        result = {"run": str(run), "errors": errors, "requestMethods": [item["method"] for item in trace if "method" in item], "screenBytes": len(output), "nativeTurnsSubmitted": native_turns, "externalModelCalls": 0, "localFixtureRequests": len(fixture_requests), "toolCallsHandledByController": len(tool_calls), "turnCompleted": "turn/completed" in native_events, "nativeScreenContainsStream": b"NATIVE_VIEW_CONTROLLER_STREAM_OK" in output}
        (run / "result.json").write_text(json.dumps(result, indent=2), encoding="utf-8")
        print(json.dumps(result, indent=2))


if __name__ == "__main__":
    asyncio.run(main())
