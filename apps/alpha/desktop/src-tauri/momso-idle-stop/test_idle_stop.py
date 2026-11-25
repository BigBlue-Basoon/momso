import datetime as dt
import http.server
import json
import subprocess
import sys
from pathlib import Path
import tempfile
import threading
import unittest
import uuid
from unittest.mock import Mock, patch

import idle_stop
from idle_stop import Compute, IdleAttemptError, IdleError, IdleStop, Server, configured_idle_seconds, wait_for_recovery


class State:
    def __init__(self):
        self.server_token = uuid.uuid4().hex
        self.cloud_token = uuid.uuid4().hex
        self.stop_calls = 0
        self.changed = False
        # Like the real server: POST /maintenance starts a backup that keeps
        # /activity not ready and refuses release with 409 for a while.
        self.gate_backup_requests = 0
        self.backup_left = 0
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
            view = dict(state.view); self.server.tick(); self.write(view); return
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
            if not active and state.backup_left:
                self.server.tick()
                raw = json.dumps({"error": "backup_in_progress"}).encode()
                self.send_response(409); self.send_header("Content-Length", str(len(raw))); self.end_headers(); self.wfile.write(raw); return
            state.view["maintenance"] = active; state.view["quiescent"] = active
            if active and state.changed: state.view["busy"] = True; state.view["activeTurns"] = 1
            if active and state.gate_backup_requests:
                state.backup_left = state.gate_backup_requests
                state.view.update(busy=True, quiescent=False, shutdownReady=False, activeBackups=1)
            self.write({"maintenance":active}); return
        if self.path == "/compute/v1/projects/fixture-project/zones/asia-northeast3-a/instances/fixture-vm/stop":
            assert self.headers.get("Authorization") == "Bearer " + state.cloud_token
            state.stop_calls += 1; self.write({"name":"operation-fixture"}); return
        self.send_error(404)


class FixtureServer(http.server.ThreadingHTTPServer):
    def tick(self):
        state = self.fixture
        if state.backup_left:
            state.backup_left -= 1
            if not state.backup_left:
                state.view.update(busy=False, quiescent=state.view["maintenance"], shutdownReady=True, activeBackups=0)


class IdleTests(unittest.TestCase):
    def test_configured_idle_threshold_preserves_default_and_cli_precedence(self):
        self.assertEqual(configured_idle_seconds(None, {}), 1800)
        self.assertEqual(configured_idle_seconds(None, {"MOMSO_IDLE_SECONDS": "300"}), 300)
        self.assertEqual(configured_idle_seconds(42, {"MOMSO_IDLE_SECONDS": "invalid"}), 42)
        for bad in ("0", "-1", "1.5", "", " 300"):
            with self.assertRaisesRegex(ValueError, "invalid_idle_seconds"):
                configured_idle_seconds(None, {"MOMSO_IDLE_SECONDS": bad})

    def setUp(self):
        self.temp = tempfile.TemporaryDirectory(); root = Path(self.temp.name)
        self.fixture = State(); self.http = FixtureServer(("127.0.0.1", 0), Handler); self.http.fixture = self.fixture
        self.thread = threading.Thread(target=self.http.serve_forever, daemon=True); self.thread.start()
        port = self.http.server_port
        token = root / "server-token"; token.write_text(self.fixture.server_token)
        self.server = Server(token, port)
        self.compute = Compute(); self.compute.METADATA = f"http://127.0.0.1:{port}/computeMetadata/v1/"; self.compute.API = f"http://127.0.0.1:{port}/compute/v1"
        self.monitor = IdleStop(self.server, self.compute, root / "state", 1800)
        self.clock = [0.0]; self.monitor.clock = lambda: self.clock[0]
        def fake_sleep(seconds): self.clock[0] += seconds
        self.monitor.sleep = fake_sleep
    def tearDown(self): self.http.shutdown(); self.http.server_close(); self.thread.join(); self.temp.cleanup()
    def test_idle_stop_and_next_boot_resume(self):
        self.assertEqual(self.monitor.step(), "stop_requested"); self.assertEqual(self.fixture.stop_calls, 1)
        self.assertTrue(self.fixture.view["maintenance"])
        self.monitor.boot_id = str(uuid.uuid4())
        self.assertEqual(self.monitor.recover(), "resumed"); self.assertFalse(self.fixture.view["maintenance"])
        for path in self.monitor.state.iterdir():
            self.assertNotIn(self.fixture.server_token.encode(), path.read_bytes())
            self.assertNotIn(self.fixture.cloud_token.encode(), path.read_bytes())
    def test_waits_for_backup_after_closing_admission(self):
        original = self.server.call
        reads = []
        def call(path, method="GET"):
            value = original(path, method)
            if path == "/activity" and value["maintenance"]:
                reads.append(1)
                if len(reads) == 1:
                    value.update({"busy": True, "quiescent": False, "shutdownReady": False,
                                  "activeBackups": 1, "backupStatus": "pending"})
                else:
                    value.update({"busy": False, "quiescent": True, "shutdownReady": True,
                                  "activeBackups": 0, "backupStatus": "saved"})
            return value
        self.server.call = call
        with patch.object(idle_stop.time, "sleep"):
            self.assertEqual(self.monitor.step(), "stop_requested")
        self.assertEqual(len(reads), 2)
        self.assertEqual(self.fixture.stop_calls, 1)
    def test_opt_in_shorter_idle_still_requires_backup_and_maintenance(self):
        self.fixture.view["lastActivityAt"] = (dt.datetime.now(dt.timezone.utc) - dt.timedelta(minutes=6)).isoformat()
        self.assertEqual(self.monitor.step(), "not_ready")
        self.monitor.idle_seconds = configured_idle_seconds(None, {"MOMSO_IDLE_SECONDS": "300"})
        self.fixture.view["shutdownReady"] = False
        self.assertEqual(self.monitor.step(), "not_ready")
        self.fixture.view["shutdownReady"] = True
        self.assertEqual(self.monitor.step(), "stop_requested")
        self.assertTrue(self.fixture.view["maintenance"])
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
    def test_backup_started_by_closing_gate_is_waited_out_then_stops(self):
        # S174: the real server answers right after the gate closes with a
        # running backup. Earlier code reopened (409) and never stopped.
        self.fixture.gate_backup_requests = 4
        self.assertEqual(self.monitor.step(), "stop_requested")
        self.assertEqual(self.fixture.stop_calls, 1)
        self.assertTrue(self.fixture.view["maintenance"])
        self.assertGreater(self.clock[0], 0)

    def test_real_activity_during_gate_backup_reopens_after_backup(self):
        self.fixture.gate_backup_requests = 3
        self.fixture.changed = True
        self.assertEqual(self.monitor.step(), "activity_changed")
        self.assertEqual(self.fixture.stop_calls, 0)
        self.assertFalse(self.fixture.view["maintenance"]); self.assertFalse(self.monitor.marker.exists())

    def test_backup_outlasting_window_fails_with_reason_then_reopens(self):
        self.monitor.settle_seconds = 10
        self.fixture.gate_backup_requests = 100
        with self.assertRaises(IdleAttemptError) as caught:
            self.monitor.step()
        self.assertEqual(str(caught.exception), "release:HTTP409:backup_in_progress")
        self.assertTrue(self.fixture.view["maintenance"]); self.assertTrue(self.monitor.marker.exists())
        self.assertEqual(self.fixture.stop_calls, 0)
        self.fixture.backup_left = 1
        self.assertEqual(self.monitor.step(), "resumed")
        self.assertFalse(self.fixture.view["maintenance"]); self.assertFalse(self.monitor.marker.exists())

    def test_backup_never_ready_within_window_reopens_as_settle_timeout(self):
        self.monitor.settle_seconds = 10
        self.fixture.gate_backup_requests = 1
        self.fixture.view["shutdownReady"] = True
        real_call = self.server.call
        def backup_failed(path, method="GET"):
            result = real_call(path, method)
            if path == "/activity" and self.fixture.view["maintenance"]:
                result = dict(result, shutdownReady=False, quiescent=False)
            return result
        self.server.call = backup_failed
        self.assertEqual(self.monitor.step(), "settle_timeout")
        self.assertEqual(self.fixture.stop_calls, 0)
        self.assertFalse(self.fixture.view["maintenance"])

    def test_http_reason_never_copies_unknown_body(self):
        import io, urllib.error
        secret = self.fixture.server_token
        for body, expected in ((b'{"error":"backup_in_progress"}', "x:HTTP409:backup_in_progress"),
                               (json.dumps({"error": "Bearer " + secret}).encode(), "x:HTTP409"),
                               (secret.encode(), "x:HTTP409")):
            error = urllib.error.HTTPError("http://127.0.0.1/", 409, "conflict " + secret, {}, io.BytesIO(body))
            self.assertEqual(str(IdleAttemptError("x", error)), expected)

    def test_unconfirmed_cloud_request_keeps_gate_until_recovery(self):
        self.compute.stop_self = lambda: (_ for _ in ()).throw(OSError("fixture"))
        with self.assertRaisesRegex(IdleAttemptError, "compute_stop:OSError"): self.monitor.step()
        self.assertTrue(self.fixture.view["maintenance"]); self.assertTrue(self.monitor.marker.exists())
        self.assertEqual(self.monitor.recover(), "stop_pending")
        status = json.loads(self.monitor.status.read_text())
        status["attemptedAt"] = (dt.datetime.now(dt.timezone.utc) - dt.timedelta(minutes=6)).isoformat()
        self.monitor.status.write_text(json.dumps(status))
        self.assertEqual(self.monitor.recover(), "resumed")
        self.assertFalse(self.fixture.view["maintenance"])

    def test_failed_gate_releases_and_next_idle_attempt_stops(self):
        real_call = self.server.call
        def fail_activity_after_gate(path, method="GET"):
            if path == "/activity" and self.fixture.view["maintenance"]:
                raise OSError("secret must never appear")
            return real_call(path, method)
        self.server.call = fail_activity_after_gate
        with self.assertRaises(IdleAttemptError) as caught:
            self.monitor.step()
        self.assertEqual(str(caught.exception), "activity:OSError")
        self.server.call = real_call
        self.assertEqual(self.monitor.step(), "resumed")
        self.assertFalse(self.fixture.view["maintenance"])
        self.assertFalse(self.monitor.marker.exists())
        # The real server resets lastActivityAt when it reopens admission.
        self.fixture.view["lastActivityAt"] = dt.datetime.now(dt.timezone.utc).isoformat()
        self.assertEqual(self.monitor.step(), "not_ready")
        self.fixture.view["lastActivityAt"] = (dt.datetime.now(dt.timezone.utc) - dt.timedelta(hours=1)).isoformat()
        self.assertEqual(self.monitor.step(), "stop_requested")
        self.assertEqual(self.fixture.stop_calls, 1)

    def test_recent_stop_request_waits_before_reopening_on_same_boot(self):
        self.assertEqual(self.monitor.step(), "stop_requested")
        self.monitor.report("stop_requested")
        self.assertEqual(self.monitor.step(), "stop_pending")
        self.assertTrue(self.fixture.view["maintenance"])
        status = json.loads(self.monitor.status.read_text())
        status["attemptedAt"] = (dt.datetime.now(dt.timezone.utc) - dt.timedelta(minutes=6)).isoformat()
        self.monitor.status.write_text(json.dumps(status))
        self.assertEqual(self.monitor.step(), "resumed")
        self.assertFalse(self.fixture.view["maintenance"])

    def test_diagnostic_status_contains_only_safe_code(self):
        self.monitor.report("error:activity:OSError")
        status = json.loads(self.monitor.status.read_text())
        self.assertEqual(set(status), {"attemptedAt", "result"})
        self.assertEqual(status["result"], "error:activity:OSError")
        self.assertNotIn(self.fixture.server_token, json.dumps(status))
        self.assertNotIn(self.fixture.cloud_token, json.dumps(status))

    def test_startup_recovery_error_is_logged_and_retried(self):
        monitor = Mock()
        args = ["idle-stop", "--token-file", "fixture", "--state-dir", "fixture", "--recover-only"]
        with patch.object(sys, "argv", args), patch.object(idle_stop, "IdleStop", return_value=monitor), \
             patch.object(idle_stop, "wait_for_recovery", side_effect=[OSError("private"), "resumed"]), \
             patch.object(idle_stop.time, "sleep") as sleep, patch("builtins.print") as output:
            idle_stop.main()
        output.assert_called_once_with("idle_stop_unavailable reason=recovery:OSError", flush=True)
        monitor.report.assert_called_once_with("error:recovery:OSError")
        sleep.assert_called_once_with(120)

    def test_other_process_holds_update_lock_then_idle_can_stop(self):
        code = ("import fcntl,sys; f=open(sys.argv[1],'a+b'); "
                "fcntl.flock(f,fcntl.LOCK_EX); print('locked',flush=True); sys.stdin.readline()")
        child = subprocess.Popen([sys.executable, "-c", code, str(self.monitor.lock)],
                                 stdin=subprocess.PIPE, stdout=subprocess.PIPE, text=True)
        try:
            self.assertEqual(child.stdout.readline().strip(), "locked")
            self.assertEqual(self.monitor.step(), "lifecycle_busy")
            child.stdin.write("done\n"); child.stdin.flush()
            self.assertEqual(child.wait(timeout=5), 0)
            self.assertEqual(self.monitor.step(), "stop_requested")
        finally:
            if child.poll() is None: child.kill(); child.wait()
            child.stdin.close(); child.stdout.close()

    def test_updater_waiting_on_failed_marker_can_continue_after_recovery(self):
        sys.path.insert(0, str(Path(__file__).resolve().parents[1] / "momso-server-update"))
        try:
            from server_admission import ServerAdmission
            admission = ServerAdmission(self.server.token_file, self.monitor.state,
                                        port=self.http.server_port, timeout=1)
            original_call = self.server.call
            def fail_after_gate(path, method="GET"):
                if path == "/activity" and self.fixture.view["maintenance"]: raise OSError("test")
                return original_call(path, method)
            self.server.call = fail_after_gate
            with self.assertRaises(IdleAttemptError): self.monitor.step()
            self.assertFalse(admission.acquire())
            self.server.call = original_call
            self.assertEqual(self.monitor.step(), "resumed")
            self.assertTrue(admission.acquire())
            self.assertEqual(self.monitor.step(), "lifecycle_busy")
            admission.release()
        finally:
            sys.path.pop(0)
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
        monitor.recover.side_effect = ["lifecycle_busy", "resumed"]
        with patch("idle_stop.time.sleep"):
            self.assertEqual(wait_for_recovery(monitor, 1), "resumed")
        monitor.recover.side_effect = IdleError("marker_invalid")
        with self.assertRaisesRegex(IdleError, "marker_invalid"):
            wait_for_recovery(monitor, 1)


if __name__ == "__main__": unittest.main()
