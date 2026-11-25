"""Opt-in: a real momso-server process and a real idle-stop process together.

Earlier idle-stop tests answered /maintenance and /activity from a fake HTTP
handler that never started a backup when admission closed. The real server
increments the backup generation on POST /maintenance and wakes the object
backup, so /activity reports a running backup right after the gate closes.
This test uses the real server binary and a root-owned test backup agent
(fake_backup_agent.py) so that interaction is exercised. Only the Compute
stop call is replaced; it records the call instead of contacting Google.

    MOMSO_IDLE_REAL_SERVER=/path/to/momso-server \
    MOMSO_IDLE_REAL_AGENT=/opt/<root-owned dir>/agent.py \
    python3 -m unittest test_real_idle_stop -v
"""
import json
import os
from pathlib import Path
import socket
import subprocess
import sys
import tempfile
import time
import unittest
import urllib.request
import uuid

HERE = Path(__file__).resolve().parent

# The idle-stop process imports the real script and swaps only Compute.
RUNNER = r"""
import json, sys
from pathlib import Path
sys.path.insert(0, sys.argv.pop(1))
import idle_stop
calls = Path(sys.argv.pop(1))
class FakeCompute:
    METADATA = "http://127.0.0.1:9/"
    API = "http://127.0.0.1:9/"
    def stop_self(self):
        with calls.open("a") as file: file.write("stop\n")
        return True
idle_stop.Compute = FakeCompute
idle_stop.main()
"""


@unittest.skipUnless(os.environ.get("MOMSO_IDLE_REAL_SERVER") and os.environ.get("MOMSO_IDLE_REAL_AGENT"),
                     "requires a real server binary and a root-owned test backup agent")
class RealIdleStopTests(unittest.TestCase):
    def start(self, delays, settle_seconds):
        self.temp = tempfile.TemporaryDirectory(prefix="momso-idle-real-")
        root = Path(self.temp.name)
        (root / "fake-agent-delays").write_text(",".join(str(value) for value in delays))
        token = uuid.uuid4().hex
        self.token = token
        token_file = root / "token"; token_file.write_text(token)
        with socket.socket() as sock:
            sock.bind(("127.0.0.1", 0)); self.port = sock.getsockname()[1]
        env = dict(os.environ, MOMSO_SERVER_DATA_DIR=str(root / "data"), MOMSO_SERVER_LISTEN=f"127.0.0.1:{self.port}",
                   MOMSO_SERVER_TOKEN=token, MOMSO_CODEX_PATH="/bin/false", MOMSO_BACKUP_AGENT=os.environ["MOMSO_IDLE_REAL_AGENT"])
        env.pop("MOMSO_IDLE_SECONDS", None)
        self.server = subprocess.Popen([os.environ["MOMSO_IDLE_REAL_SERVER"]], env=env,
                                       stdout=subprocess.DEVNULL, stderr=subprocess.DEVNULL)
        self.addCleanup(self.stop_process, self.server)
        deadline = time.monotonic() + 60
        while True:
            try:
                view = self.activity()
                if view.get("shutdownReady") is True and view.get("objectBackup", {}).get("state") == "saved":
                    break
            except OSError:
                pass
            if time.monotonic() >= deadline:
                self.fail("server did not finish its first backup")
            time.sleep(0.2)
        self.calls = root / "stop-calls"
        self.log = (root / "idle-stop.log").open("w")
        self.idle = subprocess.Popen(
            [sys.executable, "-c", RUNNER, str(HERE), str(self.calls), "--token-file", str(token_file),
             "--state-dir", str(root / "state"), "--port", str(self.port), "--idle-seconds", "60",
             "--poll-seconds", "5", "--settle-seconds", str(settle_seconds)],
            stdout=self.log, stderr=subprocess.STDOUT, env={"PATH": "/usr/bin:/bin"})
        self.addCleanup(self.stop_process, self.idle)
        self.addCleanup(self.temp.cleanup)
        self.addCleanup(self.log.close)
        self.root = root

    @staticmethod
    def stop_process(process):
        if process.poll() is None:
            process.terminate()
            try: process.wait(timeout=10)
            except subprocess.TimeoutExpired: process.kill(); process.wait()

    def activity(self):
        request = urllib.request.Request(f"http://127.0.0.1:{self.port}/activity",
                                         headers={"Authorization": "Bearer " + self.token})
        with urllib.request.urlopen(request, timeout=5) as response:
            return json.load(response)

    def wait_for_stop(self, seconds):
        deadline = time.monotonic() + seconds
        while time.monotonic() < deadline:
            if self.calls.exists() and self.calls.read_text() == "stop\n":
                return True
            self.assertIsNone(self.idle.poll(), self.output())
            time.sleep(1)
        return False

    def output(self):
        self.log.flush()
        return (self.root / "idle-stop.log").read_text()

    def test_backup_started_by_closing_admission_does_not_block_idle_stop(self):
        # The first backup (server start) is instant; the one the gate starts takes 4 s.
        self.start([0, 4], settle_seconds=120)
        self.assertTrue(self.wait_for_stop(150), self.output())
        status = json.loads((self.root / "state" / "idle-stop-status.json").read_text())
        self.assertEqual(status["result"], "stop_requested")
        view = self.activity()
        # The gate stays closed for the stopping VM; next boot recovery reopens it.
        self.assertIs(view["maintenance"], True)
        self.assertEqual(int((self.root / "fake-agent-runs").read_text()), 2)
        self.assertNotIn("idle_stop_unavailable", self.output())
        self.assertNotIn(self.token, self.output())

    def test_failed_attempt_reopens_records_reason_and_next_attempt_stops(self):
        # The gate's first backup outlasts the settle window and the release
        # retries, so the first attempt fails while the backup is still running.
        self.start([0, 25, 0], settle_seconds=8)
        self.assertTrue(self.wait_for_stop(240), self.output())
        log = self.output()
        self.assertIn("idle_stop_unavailable reason=release:HTTP409:backup_in_progress", log)
        self.assertIn("idle_stop_result result=resumed", log)
        self.assertEqual(int((self.root / "fake-agent-runs").read_text()), 3)
        self.assertEqual(json.loads((self.root / "state" / "idle-stop-status.json").read_text())["result"], "stop_requested")
        self.assertNotIn(self.token, log)


if __name__ == "__main__":
    unittest.main()
