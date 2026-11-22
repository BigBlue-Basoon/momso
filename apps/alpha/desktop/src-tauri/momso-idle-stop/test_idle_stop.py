import datetime as dt
import http.server
import json
from pathlib import Path
import tempfile
import threading
import unittest
import uuid
from unittest.mock import Mock, patch

from idle_stop import Compute, IdleError, IdleStop, Server, wait_for_recovery


class State:
    def __init__(self):
        self.server_token = uuid.uuid4().hex
        self.cloud_token = uuid.uuid4().hex
        self.stop_calls = 0
        self.changed = False
        self.view = {"shutdownReady": True, "busy": False, "maintenance": False,
            "quiescent": False, "activeTurns": 0, "pendingApprovals": 0,
            "pendingInputs": 0, "activeWrites": 0, "githubLogins": 0,
            "admittedRequests": 0, "providerLogin": False,
            "lastActivityAt": (dt.datetime.now(dt.timezone.utc) - dt.timedelta(hours=1)).isoformat()}


class Handler(http.server.BaseHTTPRequestHandler):
    def log_message(self, *args): pass
    def write(self, obj):
        raw = obj if isinstance(obj, bytes) else json.dumps(obj).encode()
        self.send_response(200); self.send_header("Content-Length", str(len(raw))); self.end_headers(); self.wfile.write(raw)
    def do_GET(self):
        state = self.server.fixture
        if self.path == "/activity":
            assert self.headers.get("Authorization") == "Bearer " + state.server_token
            self.write(state.view); return
        if self.path.startswith("/computeMetadata/v1/"):
            assert self.headers.get("Metadata-Flavor") == "Google"
            path = self.path.removeprefix("/computeMetadata/v1/")
            values = {"project/project-id": b"fixture-project", "instance/zone": b"projects/0/zones/asia-northeast3-a", "instance/name": b"fixture-vm", "instance/service-accounts/default/token": json.dumps({"access_token":state.cloud_token}).encode()}
            self.write(values[path]); return
        self.send_error(404)
    def do_POST(self):
        state = self.server.fixture
        if self.path in ("/maintenance", "/maintenance/release"):
            assert self.headers.get("Authorization") == "Bearer " + state.server_token
            active = self.path == "/maintenance"
            state.view["maintenance"] = active; state.view["quiescent"] = active
            if active and state.changed: state.view["busy"] = True
            self.write({"maintenance":active}); return
        if self.path == "/compute/v1/projects/fixture-project/zones/asia-northeast3-a/instances/fixture-vm/stop":
            assert self.headers.get("Authorization") == "Bearer " + state.cloud_token
            state.stop_calls += 1; self.write({"name":"operation-fixture"}); return
        self.send_error(404)


class IdleTests(unittest.TestCase):
    def setUp(self):
        self.temp = tempfile.TemporaryDirectory(); root = Path(self.temp.name)
        self.fixture = State(); self.http = http.server.ThreadingHTTPServer(("127.0.0.1", 0), Handler); self.http.fixture = self.fixture
        self.thread = threading.Thread(target=self.http.serve_forever, daemon=True); self.thread.start()
        port = self.http.server_port
        token = root / "server-token"; token.write_text(self.fixture.server_token)
        self.server = Server(token, port)
        self.compute = Compute(); self.compute.METADATA = f"http://127.0.0.1:{port}/computeMetadata/v1/"; self.compute.API = f"http://127.0.0.1:{port}/compute/v1"
        self.monitor = IdleStop(self.server, self.compute, root / "state", 1800)
    def tearDown(self): self.http.shutdown(); self.http.server_close(); self.thread.join(); self.temp.cleanup()
    def test_idle_stop_and_next_boot_resume(self):
        self.assertEqual(self.monitor.step(), "stop_requested"); self.assertEqual(self.fixture.stop_calls, 1)
        self.assertTrue(self.fixture.view["maintenance"]); self.assertEqual(self.monitor.step(), "stop_pending")
        self.assertEqual(self.monitor.recover(), "stop_pending")
        self.monitor.boot_id = str(uuid.uuid4())
        self.assertEqual(self.monitor.recover(), "resumed"); self.assertFalse(self.fixture.view["maintenance"])
        for path in self.monitor.state.iterdir():
            self.assertNotIn(self.fixture.server_token.encode(), path.read_bytes())
            self.assertNotIn(self.fixture.cloud_token.encode(), path.read_bytes())
    def test_backup_not_ready_busy_missing_or_recent_never_stop(self):
        for key, value in (("shutdownReady", False), ("busy", True), ("activeTurns", 1), ("pendingInputs", 1), ("providerLogin", True), ("activeWrites", 1), ("githubLogins", 1), ("admittedRequests", 1), ("lastActivityAt", dt.datetime.now(dt.timezone.utc).isoformat())):
            before = self.fixture.view[key]; self.fixture.view[key] = value
            self.assertEqual(self.monitor.step(), "not_ready", key); self.fixture.view[key] = before
        del self.fixture.view["shutdownReady"]
        self.assertEqual(self.monitor.step(), "not_ready"); self.assertEqual(self.fixture.stop_calls, 0)
    def test_activity_race_reopens_without_stopping(self):
        self.fixture.changed = True
        self.assertEqual(self.monitor.step(), "activity_changed")
        self.assertEqual(self.fixture.stop_calls, 0); self.assertFalse(self.fixture.view["maintenance"])
    def test_unconfirmed_cloud_request_keeps_gate_until_recovery(self):
        self.compute.stop_self = lambda: (_ for _ in ()).throw(OSError("fixture"))
        with self.assertRaisesRegex(IdleError, "stop_or_gate_unconfirmed"): self.monitor.step()
        self.assertTrue(self.fixture.view["maintenance"]); self.assertTrue(self.monitor.marker.exists())
        self.assertEqual(self.monitor.recover(), "stop_pending")
        self.monitor.boot_id = str(uuid.uuid4())
        self.assertEqual(self.monitor.recover(), "resumed")
    def test_damaged_recovery_marker_never_releases_gate(self):
        self.monitor.marker.write_text("bad")
        self.fixture.view["maintenance"] = True
        with self.assertRaisesRegex(IdleError, "marker_invalid"): self.monitor.recover()
        self.assertTrue(self.fixture.view["maintenance"])
        self.assertEqual(self.fixture.stop_calls, 0)

    def test_unreadable_token_never_stops(self):
        self.server.token_file.unlink()
        with self.assertRaises(OSError): self.monitor.step()
        self.assertEqual(self.fixture.stop_calls, 0)

    def test_other_lifecycle_operation_prevents_stop(self):
        import fcntl
        with self.monitor.lock.open("a+b") as held:
            fcntl.flock(held, fcntl.LOCK_EX | fcntl.LOCK_NB)
            self.assertEqual(self.monitor.step(), "lifecycle_busy")
        self.assertEqual(self.fixture.stop_calls, 0)

    def test_updater_lock_waits_without_failing_or_stopping(self):
        with self.monitor.lock.open("a+b") as held:
            import fcntl
            fcntl.flock(held, fcntl.LOCK_EX | fcntl.LOCK_NB)
            def release_lock(_seconds):
                self.assertEqual(self.fixture.stop_calls, 0)
                self.assertFalse(self.monitor.marker.exists())
                fcntl.flock(held, fcntl.LOCK_UN)
            with patch("idle_stop.time.sleep", side_effect=release_lock) as sleep:
                self.assertEqual(wait_for_recovery(self.monitor, 1), "no_recovery")
            sleep.assert_called_once_with(1)
        self.assertEqual(self.fixture.stop_calls, 0)

    def test_recovery_wait_preserves_pending_and_errors(self):
        monitor = Mock()
        monitor.recover.side_effect = ["lifecycle_busy", "stop_pending"]
        with patch("idle_stop.time.sleep"):
            self.assertEqual(wait_for_recovery(monitor, 1), "stop_pending")
        monitor.recover.side_effect = IdleError("marker_invalid")
        with self.assertRaisesRegex(IdleError, "marker_invalid"):
            wait_for_recovery(monitor, 1)


if __name__ == "__main__": unittest.main()
