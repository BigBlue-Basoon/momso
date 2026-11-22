import http.server
import json
from pathlib import Path
import tempfile
import threading
import unittest
import uuid
from server_admission import ServerAdmission


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
        self.reply({"maintenance": self.server.maintenance})


class AdmissionTests(unittest.TestCase):
    def setUp(self):
        self.tmp = tempfile.TemporaryDirectory(); root = Path(self.tmp.name); token = uuid.uuid4().hex
        self.server = http.server.ThreadingHTTPServer(("127.0.0.1", 0), Handler)
        self.server.fixture_token = token; self.server.maintenance = False; self.server.busy = False; self.server.ready = False
        self.thread = threading.Thread(target=self.server.serve_forever, daemon=True); self.thread.start()
        self.file = root / "token"; self.file.write_text(token)
        self.admission = ServerAdmission(self.file, root / "lifecycle", port=self.server.server_port, timeout=0)
    def tearDown(self): self.server.shutdown(); self.server.server_close(); self.thread.join(); self.tmp.cleanup()
    def test_backup_not_ready_releases_without_installing(self):
        self.admission.timeout = 60
        self.assertFalse(self.admission.acquire()); self.assertFalse(self.server.maintenance)
    def test_ready_gate_is_held_until_release(self):
        self.server.ready = True
        self.assertTrue(self.admission.acquire()); self.assertTrue(self.server.maintenance)
        self.admission.release(); self.assertFalse(self.server.maintenance)
    def test_existing_maintenance_never_takes_over(self):
        self.server.maintenance = True
        self.assertFalse(self.admission.acquire()); self.assertTrue(self.server.maintenance)


if __name__ == "__main__": unittest.main()
