import http.server
import json
from pathlib import Path
import tempfile
import threading
import unittest
import uuid
from unittest.mock import patch
from server_admission import AdmissionError, ServerAdmission


class Handler(http.server.BaseHTTPRequestHandler):
    def log_message(self, *args): pass
    def reply(self, value):
        data = json.dumps(value).encode(); self.send_response(200); self.send_header("Content-Length", str(len(data))); self.end_headers(); self.wfile.write(data)
    def do_GET(self):
        assert self.headers["Authorization"] == "Bearer " + self.server.fixture_token
        self.reply({"maintenance": self.server.maintenance, "quiescent": self.server.maintenance and not self.server.busy, "maintenanceReady": self.server.ready, "maintenanceReason": None if self.server.ready else "backup_not_integrated"})
    def do_POST(self):
        assert self.headers["Authorization"] == "Bearer " + self.server.fixture_token
        self.server.maintenance = self.path == "/maintenance"
        if self.server.drop_replies:
            # The request took effect but its answer never arrives.
            self.server.drop_replies -= 1
            self.close_connection = True
            return
        self.reply({"maintenance": self.server.maintenance, "closedAt": 123 if self.server.maintenance else None})


class AdmissionTests(unittest.TestCase):
    def setUp(self):
        self.tmp = tempfile.TemporaryDirectory(); root = Path(self.tmp.name); token = uuid.uuid4().hex
        self.server = http.server.ThreadingHTTPServer(("127.0.0.1", 0), Handler)
        self.server.fixture_token = token; self.server.maintenance = False; self.server.busy = False; self.server.ready = False
        self.server.drop_replies = 0
        self.thread = threading.Thread(target=self.server.serve_forever, daemon=True); self.thread.start()
        self.file = root / "token"; self.file.write_text(token)
        self.admission = ServerAdmission(self.file, root / "lifecycle", port=self.server.server_port, timeout=0)
    def tearDown(self): self.server.shutdown(); self.server.server_close(); self.thread.join(); self.tmp.cleanup()
    def test_quiescent_server_does_not_require_backup_ready(self):
        self.admission.timeout = 60
        self.assertTrue(self.admission.acquire()); self.assertTrue(self.server.maintenance)
        self.admission.release(); self.assertFalse(self.server.maintenance)
    def test_busy_server_is_not_stopped(self):
        self.server.busy = True
        self.assertFalse(self.admission.acquire()); self.assertFalse(self.server.maintenance)
    def test_ready_gate_is_held_until_release(self):
        self.server.ready = True
        self.assertTrue(self.admission.acquire()); self.assertTrue(self.server.maintenance)
        self.assertEqual(self.admission.closed_at, 123)
        self.admission.release(); self.assertFalse(self.server.maintenance)
        self.assertIsNone(self.admission.closed_at)
    def test_existing_maintenance_never_takes_over(self):
        self.server.maintenance = True
        self.assertFalse(self.admission.acquire()); self.assertTrue(self.server.maintenance)
    def test_interrupted_owner_can_reacquire_during_recovery(self):
        self.server.maintenance = True
        self.assertTrue(self.admission.reacquire_for_recovery())
        self.assertTrue(self.server.maintenance)
        self.admission.release()
        self.assertFalse(self.server.maintenance)
    def test_interrupted_recovery_keeps_busy_gate_closed(self):
        self.server.maintenance = True
        self.server.busy = True
        self.assertFalse(self.admission.reacquire_for_recovery())
        self.assertTrue(self.server.maintenance)
    def test_offline_recovery_holds_lifecycle_lock_until_release(self):
        self.server.maintenance = True
        self.assertTrue(self.admission.acquire_offline_for_recovery())
        competing = ServerAdmission(self.file, self.admission.directory, port=self.server.server_port, timeout=0)
        self.assertFalse(competing.acquire_offline_for_recovery())
        self.admission.release()
        self.assertFalse(self.server.maintenance)

    def test_recovery_reuses_own_lock_after_release_failure(self):
        self.assertTrue(self.admission.acquire())
        with patch.object(self.admission, '_request', side_effect=OSError('fixture')):
            with self.assertRaises(OSError): self.admission.release()
        self.assertTrue(self.admission.reacquire_for_recovery())
        self.admission.release()
        self.assertFalse(self.server.maintenance)

    def test_recovery_reacquires_if_release_response_was_lost(self):
        self.assertTrue(self.admission.acquire())
        self.server.maintenance = False
        self.assertTrue(self.admission.reacquire_for_recovery())
        self.assertTrue(self.server.maintenance)
        self.admission.release()

    def test_lost_maintenance_reply_is_adopted_by_the_same_owner(self):
        self.admission.timeout = 60
        self.server.drop_replies = 1
        with self.assertRaises(AdmissionError): self.admission.acquire()
        self.assertTrue(self.server.maintenance); self.assertTrue(self.admission.pending())
        # Earlier code kept the lock and answered False here forever.
        self.assertTrue(self.admission.acquire())
        self.admission.release()
        self.assertFalse(self.server.maintenance); self.assertFalse(self.admission.pending())

    def test_lost_maintenance_reply_is_adopted_after_worker_restart(self):
        self.admission.timeout = 60
        self.server.drop_replies = 1
        with self.assertRaises(AdmissionError): self.admission.acquire()
        self.admission._unlock()  # the process exits; flock goes with it
        restarted = ServerAdmission(self.file, self.admission.directory, port=self.server.server_port, timeout=60)
        self.assertTrue(restarted.acquire())
        self.assertIsNone(restarted.closed_at)
        restarted.release()
        self.assertFalse(self.server.maintenance); self.assertFalse(restarted.pending())

    def test_gate_closed_without_owner_marker_is_never_adopted(self):
        self.server.maintenance = True
        self.assertFalse(self.admission.acquire()); self.assertTrue(self.server.maintenance)
        self.assertFalse(self.admission.pending())

    def test_stale_marker_with_open_gate_is_cleared(self):
        self.admission.timeout = 60
        self.admission.marker.write_text("admission")
        self.assertTrue(self.admission.acquire()); self.assertEqual(self.admission.closed_at, 123)
        self.admission.release(); self.assertFalse(self.admission.pending())

    def test_updater_recovery_reopens_gate_after_lost_reply(self):
        self.admission.timeout = 60
        updater = self.updater()
        self.server.drop_replies = 1
        with self.assertRaises(AdmissionError): self.admission.acquire()
        self.assertEqual(updater.recover(), "released")
        self.assertFalse(self.server.maintenance); self.assertFalse(self.admission.pending())
        self.assertEqual(updater.recover(), "no_recovery")

    def test_updater_recovery_reopens_gate_after_failed_release_without_record(self):
        self.admission.timeout = 60
        updater = self.updater()
        self.assertTrue(self.admission.acquire())
        with patch.object(self.admission, "_request", side_effect=OSError("fixture")):
            with self.assertRaises(OSError): self.admission.release()
        self.assertTrue(self.server.maintenance)
        self.assertEqual(updater.recover(), "released")
        self.assertFalse(self.server.maintenance)

    def test_updater_recovery_leaves_other_owners_gate(self):
        updater = self.updater()
        self.server.maintenance = True
        self.assertEqual(updater.recover(), "no_recovery")
        self.assertTrue(self.server.maintenance)

    def updater(self):
        from update import Updater
        admission = self.admission
        class Host:
            def acquire_maintenance(self): return admission.acquire()
            def release_maintenance(self): admission.release()
            def admission_pending(self): return admission.pending()
            def progress(self, event): pass
        return Updater(Path(self.tmp.name) / "updater", Path(self.tmp.name) / "unused-key", Host())


if __name__ == "__main__": unittest.main()
