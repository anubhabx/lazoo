"""Prove the restart acceptance cannot stop another daemon using the same binary."""
import json
import os
from pathlib import Path
import subprocess
import time
import uuid

BASE = Path(__file__).resolve().parent
REPO = BASE.parents[1]
run = BASE / ("run-restart-isolation-" + uuid.uuid4().hex)
run.mkdir()
env = dict(os.environ, LAZOO_DATA_ROOT=str(run / "data"))
ctl = str(REPO / "target/release/lazooctl.exe")
daemon = None
result = {"status": "FAIL", "run": str(run), "evidence": {}}


def call(*args):
    command = subprocess.run([ctl, "--json", *args], env=env, capture_output=True, timeout=20)
    command.check_returncode()
    return json.loads(command.stdout)


try:
    with (run / "daemon.stdout.log").open("wb") as stdout, (run / "daemon.stderr.log").open("wb") as stderr:
        daemon = subprocess.Popen([str(REPO / "target/release/lazood.exe")], env=env, stdout=stdout, stderr=stderr)
        for attempt in range(50):
            try:
                before = call("status")
                break
            except subprocess.CalledProcessError:
                if attempt == 49:
                    raise
                time.sleep(.1)
        with (run / "first-slice.log").open("wb") as output:
            acceptance = subprocess.run(["pwsh", "-NoProfile", "-File", str(REPO / "scripts/gate.ps1"), "-AcceptanceOnly", "-Only", "first-slice", "-OutputPath", str(BASE / "gate-first-slice-recheck.json")], cwd=REPO, stdout=output, stderr=subprocess.STDOUT, timeout=120)
        result["evidence"]["firstSlicePassed"] = acceptance.returncode == 0
        result["evidence"]["unrelatedDaemonSurvived"] = daemon.poll() is None
        result["evidence"]["unrelatedBootUnchanged"] = call("status")["bootId"] == before["bootId"]
        assert all(result["evidence"].values()), result
        result["status"] = "PASS"
except Exception as error:
    result["error"] = repr(error)
finally:
    if daemon and daemon.poll() is None:
        try:
            call("system", "shutdown")
            daemon.wait(timeout=15)
        except Exception as error:
            daemon.kill()
            daemon.wait()
            result["cleanupError"] = repr(error)
            result["status"] = "FAIL"
    (run / "result.json").write_text(json.dumps(result, indent=2), encoding="utf-8")
    print(json.dumps(result, indent=2))
raise SystemExit(result["status"] != "PASS")
