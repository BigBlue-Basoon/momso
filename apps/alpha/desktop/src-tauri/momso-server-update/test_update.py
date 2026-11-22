import hashlib
import http.server
import io
import json
import os
from pathlib import Path
import subprocess
import tarfile
import tempfile
import threading
import unittest
from types import SimpleNamespace
from unittest.mock import patch

import update
from update import Updater, UpdateError, _atomic_json


class Host:
    def __init__(self):
        self.release = "old"; self.gateway = "old-gateway"; self.sequence = 1
        self.data = "before"; self.snapshot = None; self.lease = False
        self.available = True; self.healthy = True; self.events = []
    def progress(self, event): self.events.append(event)
    def current_sequence(self): return self.sequence
    def current_release(self): return self.release
    def current_gateway(self): return self.gateway
    def acquire_maintenance(self):
        if not self.available: return False
        self.lease = True; return True
    def release_maintenance(self): self.lease = False
    def stop_server(self): pass
    def start_server(self): pass
    def server_healthy(self): return self.healthy or self.release == "old"
    def gateway_healthy(self): return True
    def snapshot_data(self): self.snapshot = self.data; return "snapshot-1"
    def restore_data(self, snapshot):
        assert snapshot == "snapshot-1"
        self.data = self.snapshot
    def activate_release(self, release): self.release = release
    def activate_gateway(self, gateway): self.gateway = gateway
    def migrate_data(self, release): self.data = "migrated"
    def record_sequence(self, sequence): self.sequence = sequence


class UpdateTests(unittest.TestCase):
    def setUp(self):
        self.tmp = tempfile.TemporaryDirectory()
        self.root = Path(self.tmp.name)
        self.server_dir = self.root / "server"; self.server_dir.mkdir()
        self.private = self.root / "fixture-key.pem"; self.public = self.root / "fixture-pub.pem"
        subprocess.run(["openssl", "genpkey", "-algorithm", "ED25519", "-out", str(self.private)], check=True, capture_output=True)
        subprocess.run(["openssl", "pkey", "-in", str(self.private), "-pubout", "-out", str(self.public)], check=True, capture_output=True)
        handler = lambda *args, **kw: http.server.SimpleHTTPRequestHandler(*args, directory=str(self.server_dir), **kw)
        self.http = http.server.ThreadingHTTPServer(("127.0.0.1", 0), handler)
        self.thread = threading.Thread(target=self.http.serve_forever, daemon=True); self.thread.start()
        self.base = "http://127.0.0.1:" + str(self.http.server_port)
        self.host = Host(); self.updater = Updater(self.root / "updater", self.public, self.host, allow_http_for_tests=True)
        self.publish()
    def tearDown(self):
        self.http.shutdown(); self.http.server_close(); self.thread.join(); self.tmp.cleanup()
    def publish(self):
        with tarfile.open(self.server_dir / "bundle.tar.gz", "w:gz") as archive:
            for name in ("momso-server", "momso_mobile_proxy.py", "momso_anywhere_vm_proxy.py", "web/index.html"):
                data = name.encode(); info = tarfile.TarInfo(name); info.size = len(data); archive.addfile(info, io.BytesIO(data))
        bundle = (self.server_dir / "bundle.tar.gz").read_bytes()
        data = {"sequence": 2, "version": "0.1.2", "bundleUrl": self.base + "/bundle.tar.gz", "bundleSha256": hashlib.sha256(bundle).hexdigest(), "gatewayImage": "registry.example/momso@sha256:" + "1" * 64}
        (self.server_dir / "manifest.json").write_text(json.dumps(data), encoding="utf8")
        subprocess.run(["openssl", "pkeyutl", "-sign", "-inkey", str(self.private), "-rawin", "-in", str(self.server_dir / "manifest.json"), "-out", str(self.server_dir / "manifest.sig")], check=True, capture_output=True)
    def install(self): return self.updater.install(self.base + "/manifest.json", self.base + "/manifest.sig")
    def test_signed_update_and_up_to_date(self):
        self.assertEqual(self.install(), "completed")
        self.assertEqual(self.host.sequence, 2); self.assertEqual(self.host.data, "migrated")
        self.assertFalse(self.host.lease)
        self.assertEqual(self.install(), "up_to_date")

    def test_development_channel_is_not_accepted_by_production(self):
        manifest = self.server_dir / 'manifest.json'
        value = json.loads(manifest.read_text(encoding='utf-8'))
        value['channel'] = 'development'
        manifest.write_text(json.dumps(value), encoding='utf-8')
        subprocess.run(['openssl', 'pkeyutl', '-sign', '-inkey', str(self.private), '-rawin', '-in', str(manifest), '-out', str(self.server_dir / 'manifest.sig')], check=True, capture_output=True)
        with self.assertRaisesRegex(UpdateError, 'manifest_invalid'):
            self.install()
        dev = Updater(self.root / 'development', self.public, self.host, allow_http_for_tests=True, channel='development')
        self.assertEqual(dev.check(self.base + '/manifest.json', self.base + '/manifest.sig')['channel'], 'development')

    def test_extract_supports_legacy_python_without_filter(self):
        class LegacyArchive:
            def __init__(self, **kwargs): self.inner = tarfile.open(**kwargs)
            def __enter__(self): return self
            def __exit__(self, *args): self.inner.close()
            def getmembers(self): return self.inner.getmembers()
            def extractall(self, path): self.inner.extractall(path)
        legacy = SimpleNamespace(open=lambda **kwargs: LegacyArchive(**kwargs), TarError=tarfile.TarError)
        output = self.root / 'legacy'
        output.mkdir()
        with patch.object(update, 'tarfile', legacy):
            update._extract((self.server_dir / 'bundle.tar.gz').read_bytes(), output)
        self.assertTrue((output / 'web/index.html').is_file())
    def test_failure_restores_release_gateway_and_data(self):
        self.host.healthy = False
        with self.assertRaisesRegex(UpdateError, "update_failed_rolled_back"): self.install()
        self.assertEqual((self.host.release, self.host.gateway, self.host.data, self.host.sequence), ("old", "old-gateway", "before", 1))
        self.assertFalse(self.host.lease)
    def test_bad_signature_or_bundle_never_switches(self):
        (self.server_dir / "manifest.sig").write_bytes(b"bad")
        with self.assertRaisesRegex(UpdateError, "signature_invalid"): self.install()
        self.assertEqual(self.host.release, "old")
        self.publish(); (self.server_dir / "bundle.tar.gz").write_bytes(b"tampered")
        with self.assertRaisesRegex(UpdateError, "digest_mismatch"): self.install()
        self.assertEqual(self.host.release, "old")
    def test_screen_disconnect_does_not_stop_rollback(self):
        self.host.healthy = False
        self.host.progress = lambda _: (_ for _ in ()).throw(RuntimeError("screen disconnected"))
        with self.assertRaisesRegex(UpdateError, "update_failed_rolled_back"): self.install()
        self.assertEqual((self.host.release, self.host.data), ("old", "before"))
        self.assertFalse(self.host.lease)

    def test_retry_after_rolled_back_release(self):
        self.host.healthy = False
        with self.assertRaisesRegex(UpdateError, "update_failed_rolled_back"): self.install()
        self.host.healthy = True
        self.assertEqual(self.install(), "completed")

    def test_failed_rollback_keeps_maintenance_closed(self):
        self.host.healthy = False
        self.host.restore_data = lambda _: (_ for _ in ()).throw(RuntimeError("fixture"))
        with self.assertRaisesRegex(UpdateError, "rollback_failed"): self.install()
        self.assertTrue(self.host.lease)
        self.assertEqual(json.loads(self.updater.state.read_text())["phase"], "switching")

    def test_busy_and_crash_recovery(self):
        self.host.available = False
        self.assertEqual(self.install(), "busy"); self.assertEqual(self.host.release, "old")
        self.host.available = True; self.host.snapshot = "before"; self.host.data = "partial"; self.host.release = "new"
        _atomic_json(self.updater.state, {"phase":"switching", "oldRelease":"old", "oldGateway":"old-gateway", "snapshot":"snapshot-1", "oldSequence":1})
        self.assertEqual(self.updater.recover(), "rolled_back")
        self.assertEqual((self.host.release, self.host.data), ("old", "before")); self.assertFalse(self.host.lease)


if __name__ == "__main__": unittest.main()
