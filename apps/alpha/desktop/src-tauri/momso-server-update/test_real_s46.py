"""Opt-in integration with a real isolated S46 server binary."""
import json
import os
from pathlib import Path
import socket
import subprocess
import tempfile
import time
import unittest
import urllib.request
import uuid
from server_admission import ServerAdmission


@unittest.skipUnless(os.environ.get("MOMSO_S55_SERVER"), "requires isolated server binary")
class RealMaintenanceTests(unittest.TestCase):
    def test_real_server_admission_allows_signed_update_when_quiescent(self):
        with tempfile.TemporaryDirectory(prefix="momso-s55-maintenance-") as directory:
            root = Path(directory); token = uuid.uuid4().hex; token_file = root / "token"; token_file.write_text(token)
            with socket.socket() as sock:
                sock.bind(("127.0.0.1", 0)); port = sock.getsockname()[1]
            environment = dict(os.environ, MOMSO_SERVER_DATA_DIR=str(root / "data"), MOMSO_SERVER_LISTEN=f"127.0.0.1:{port}", MOMSO_SERVER_TOKEN=token, MOMSO_CODEX_PATH="/bin/false")
            server = subprocess.Popen([os.environ["MOMSO_S55_SERVER"]], env=environment, stdout=subprocess.DEVNULL, stderr=subprocess.DEVNULL)
            def activity():
                req = urllib.request.Request(f"http://127.0.0.1:{port}/activity", headers={"Authorization":"Bearer " + token})
                with urllib.request.urlopen(req, timeout=2) as response: return json.load(response)
            try:
                deadline = time.monotonic() + 30
                while True:
                    try: before = activity(); break
                    except OSError:
                        if time.monotonic() >= deadline: raise
                        time.sleep(0.1)
                self.assertFalse(before["maintenanceReady"])
                admission = ServerAdmission(token_file, root / "lifecycle", port=port, timeout=15)
                self.assertTrue(admission.health())
                from systemd_host import SystemdHost
                host = SystemdHost(root / "host", root / "data", "unused.service", "http://127.0.0.1:1/health", admission, object(), lambda _: None)
                self.assertTrue(host.server_healthy())
                self.assertTrue(admission.acquire())
                self.assertTrue(activity()["maintenance"])
                admission.release()
                self.assertFalse(activity()["maintenance"])
                # The signed updater uses the real gate and fake release host.
                from test_update import UpdateTests
                fixture = UpdateTests("test_signed_update_and_up_to_date")
                fixture.setUp()
                try:
                    fixture.host.acquire_maintenance = admission.acquire
                    fixture.host.release_maintenance = admission.release
                    self.assertEqual(fixture.install(), "completed")
                    self.assertEqual(fixture.host.sequence, 2)
                    self.assertFalse(activity()["maintenance"])
                finally:
                    fixture.tearDown()
            finally:
                server.terminate()
                try: server.wait(timeout=10)
                except subprocess.TimeoutExpired: server.kill(); server.wait()


if __name__ == "__main__": unittest.main()
