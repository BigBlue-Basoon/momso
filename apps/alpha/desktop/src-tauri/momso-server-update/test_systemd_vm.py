"""Opt-in isolated systemd integration test. Creates only a unique runtime unit."""
import hashlib
import http.server
import io
import json
import os
from pathlib import Path
import socket
import subprocess
import tarfile
import tempfile
import threading
import unittest
import uuid

from systemd_host import SystemdHost
from update import Updater, UpdateError, _atomic_json


class Gate:
    def __init__(self): self.held = False
    def acquire(self): self.held = True; return True
    def release(self): self.held = False


class Gateway:
    def __init__(self): self.value = "fixture-old-gateway"
    def current(self): return self.value
    def activate(self, value): self.value = value
    def healthy(self): return True


@unittest.skipUnless(os.environ.get("MOMSO_RUN_SYSTEMD_TEST") == "1" and os.geteuid() == 0, "requires dedicated systemd test VM")
class SystemdIntegration(unittest.TestCase):
    def test_signed_release_real_systemd_start_and_rollback(self):
        with tempfile.TemporaryDirectory(prefix="momso-s48-test-") as directory:
            base = Path(directory); root = base / "host"; root.mkdir()
            data = base / "data"; data.mkdir(); (data / "database").write_text("fixture")
            old = root / "old"; old.mkdir(); (root / "current").symlink_to(old)
            _atomic_json(root / "installed.json", {"sequence": 1})
            with socket.socket() as sock:
                sock.bind(("127.0.0.1", 0)); service_port = sock.getsockname()[1]
            def program(ready):
                return ("#!/usr/bin/python3\nimport http.server,json\n"
                    "class H(http.server.BaseHTTPRequestHandler):\n"
                    " def do_GET(self):\n"
                    f"  self.send_response({200 if ready else 503});self.end_headers();self.wfile.write(json.dumps({{'state':'{'ready' if ready else 'failed'}'}}).encode())\n"
                    " def log_message(self,*args):pass\n"
                    f"http.server.HTTPServer(('127.0.0.1',{service_port}),H).serve_forever()\n").encode()
            (old / "momso-server").write_bytes(program(True)); (old / "momso-server").chmod(0o755)
            unit = "momso-s48-test-" + uuid.uuid4().hex + ".service"
            unit_file = base / unit
            unit_file.write_text("[Unit]\nDescription=isolated momso update test\n[Service]\nType=simple\nExecStart=" + str(root / "current" / "momso-server") + "\n")
            key = base / "private.pem"; pub = base / "public.pem"
            subprocess.run(["openssl", "genpkey", "-algorithm", "ED25519", "-out", str(key)], check=True, capture_output=True)
            subprocess.run(["openssl", "pkey", "-in", str(key), "-pubout", "-out", str(pub)], check=True, capture_output=True)
            distribution = base / "distribution"; distribution.mkdir()
            handler = lambda *args, **kwargs: http.server.SimpleHTTPRequestHandler(*args, directory=str(distribution), **kwargs)
            distribution_server = http.server.ThreadingHTTPServer(("127.0.0.1", 0), handler)
            thread = threading.Thread(target=distribution_server.serve_forever, daemon=True); thread.start()
            url = "http://127.0.0.1:" + str(distribution_server.server_port)
            gate = Gate(); gateway = Gateway(); events = []
            host = SystemdHost(root, data, unit, f"http://127.0.0.1:{service_port}/health", gate, gateway, events.append)
            updater = Updater(root, pub, host, allow_http_for_tests=True)
            def publish(sequence, ready):
                path = distribution / "bundle.tar.gz"
                with tarfile.open(path, "w:gz") as archive:
                    for name in ("momso-server", "momso_mobile_proxy.py", "momso_anywhere_vm_proxy.py", "web/index.html"):
                        value = program(ready) if name == "momso-server" else b"fixture"
                        info = tarfile.TarInfo(name); info.size = len(value); archive.addfile(info, io.BytesIO(value))
                manifest = {"sequence": sequence, "version": f"0.1.{sequence}", "bundleUrl": url + "/bundle.tar.gz", "bundleSha256": hashlib.sha256(path.read_bytes()).hexdigest(), "gatewayImage": "registry.example/momso@sha256:" + "1" * 64}
                (distribution / "manifest.json").write_text(json.dumps(manifest))
                subprocess.run(["openssl", "pkeyutl", "-sign", "-inkey", str(key), "-rawin", "-in", str(distribution / "manifest.json"), "-out", str(distribution / "manifest.sig")], check=True, capture_output=True)
            try:
                subprocess.run(["systemctl", "link", "--runtime", str(unit_file)], check=True, capture_output=True)
                host.start_server(); self.assertTrue(host.server_healthy())
                publish(2, True)
                self.assertEqual(updater.install(url + "/manifest.json", url + "/manifest.sig"), "completed")
                good = host.current_release(); self.assertNotEqual(good, str(old)); self.assertTrue(host.server_healthy())
                publish(3, False)
                with self.assertRaisesRegex(UpdateError, "update_failed_rolled_back"):
                    updater.install(url + "/manifest.json", url + "/manifest.sig")
                self.assertEqual(host.current_release(), good); self.assertEqual(host.current_sequence(), 2)
                self.assertTrue(host.server_healthy()); self.assertEqual((data / "database").read_text(), "fixture")
                self.assertFalse(gate.held)
            finally:
                subprocess.run(["systemctl", "stop", unit], check=False, capture_output=True)
                subprocess.run(["systemctl", "disable", "--runtime", unit], check=False, capture_output=True)
                subprocess.run(["systemctl", "daemon-reload"], check=False, capture_output=True)
                distribution_server.shutdown(); distribution_server.server_close(); thread.join()


if __name__ == "__main__": unittest.main()
