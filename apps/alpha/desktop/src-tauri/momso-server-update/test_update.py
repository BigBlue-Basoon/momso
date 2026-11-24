import hashlib
import http.server
import io
import json
import os
from pathlib import Path
import subprocess
import sys
import tarfile
import tempfile
import threading
import unittest
from types import SimpleNamespace
from unittest.mock import Mock, patch

import update
from cloud_run_gateway import GatewayError
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
        with self.assertRaisesRegex(UpdateError, "server_unhealthy"): self.install()
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
        with self.assertRaisesRegex(UpdateError, "server_unhealthy"): self.install()
        self.assertEqual((self.host.release, self.host.data), ("old", "before"))
        self.assertFalse(self.host.lease)

    def test_retry_after_rolled_back_release(self):
        self.host.healthy = False
        with self.assertRaisesRegex(UpdateError, "server_unhealthy"): self.install()
        self.host.healthy = True
        self.assertEqual(self.install(), "completed")

    def test_failed_rollback_keeps_maintenance_closed(self):
        self.host.healthy = False
        starts = []
        self.host.start_server = lambda: starts.append(self.host.release)
        def fail_restore(_):
            starts.clear()
            raise RuntimeError("fixture")
        self.host.restore_data = fail_restore
        with self.assertRaisesRegex(UpdateError, "rollback_failed"): self.install()
        self.assertTrue(self.host.lease)
        self.assertEqual(starts, [])
        self.assertNotEqual(self.host.release, 'old')
        self.assertEqual(json.loads(self.updater.state.read_text())["phase"], "switching")

    def test_failed_data_restore_restarts_unchanged_new_pair(self):
        starts = []
        self.host.recovery_data_identity = lambda: ('fixture', 1)
        self.host.start_server = lambda: starts.append(self.host.release)
        def fail_restore(_):
            starts.clear()
            raise RuntimeError('fixture restore failure')
        self.host.restore_data = fail_restore
        self.host.activate_gateway = lambda _: (_ for _ in ()).throw(GatewayError('gateway_probe_failed'))
        with self.assertRaisesRegex(UpdateError, 'rollback_failed'): self.install()
        self.assertEqual(len(starts), 1)
        self.assertNotEqual(starts[0], 'old')
        self.assertTrue(self.host.lease)
        self.assertEqual(self.host.events[-1]['reason'], 'rollback_failed')

    def test_failed_data_restore_does_not_restart_changed_pair(self):
        starts = []
        identity = [1]
        self.host.recovery_data_identity = lambda: identity[0]
        self.host.start_server = lambda: starts.append(self.host.release)
        def fail_restore(_):
            starts.clear()
            identity[0] = 2
            raise RuntimeError('fixture restore failure')
        self.host.restore_data = fail_restore
        self.host.activate_gateway = lambda _: (_ for _ in ()).throw(GatewayError('gateway_probe_failed'))
        with self.assertRaises(UpdateError): self.install()
        self.assertEqual(starts, [])
        self.assertTrue(self.host.lease)

    def test_busy_and_crash_recovery(self):
        self.host.available = False
        self.assertEqual(self.install(), "busy"); self.assertEqual(self.host.release, "old")
        self.host.available = True; self.host.snapshot = "before"; self.host.data = "partial"; self.host.release = "new"
        _atomic_json(self.updater.state, {"phase":"switching", "oldRelease":"old", "oldGateway":"old-gateway", "snapshot":"snapshot-1", "oldSequence":1})
        self.assertEqual(self.updater.recover(), "rolled_back")
        self.assertEqual((self.host.release, self.host.data), ("old", "before")); self.assertFalse(self.host.lease)

    def test_permission_failure_keeps_server_running_and_skips_unchanged_gateway(self):
        starts = []
        self.host.start_server = lambda: starts.append(self.host.release)
        def deny(image):
            if image == "old-gateway": raise AssertionError("unchanged gateway must not be rolled back")
            raise GatewayError("gateway_image_permission")
        self.host.activate_gateway = deny
        with self.assertRaisesRegex(UpdateError, "gateway_image_permission"): self.install()
        self.assertEqual(starts[-1], "old")
        self.assertEqual(self.host.data, "before")
        self.assertEqual(json.loads(self.updater.state.read_text())["phase"], "rolled_back")
        self.assertTrue(next(event for event in self.host.events if event['state'] == 'rolled_back')['rolledBack'])
        self.assertEqual(self.host.events[-1], {"state": "failed", "reason": "gateway_image_permission", "code": "gateway_image_permission", "rolledBack": True, "retryable": False})

    def test_wrong_serving_traffic_rolls_back_and_is_not_completed(self):
        def mismatch(image):
            if image != 'old-gateway':
                self.host.gateway = 'unexpected-new'
                raise GatewayError('gateway_traffic_mismatch')
            self.host.gateway = image
        self.host.activate_gateway = mismatch
        with self.assertRaisesRegex(UpdateError, 'gateway_traffic_mismatch'):
            self.install()
        self.assertEqual(self.host.gateway, 'old-gateway')
        self.assertEqual(json.loads(self.updater.state.read_text())['phase'], 'rolled_back')
        self.assertNotIn('completed', [event['state'] for event in self.host.events])

    def test_three_gateway_paths_report_same_mismatch_and_verified_rollback(self):
        root = Path(__file__).resolve().parents[5]
        sys.path.insert(0, str(root / 'scripts'))
        try:
            import momso_anywhere_install as pc
            previous = {'traffic': {'old-rev': 100}}
            def activate(release, **_kwargs):
                if not isinstance(release, dict): raise pc.GatewayError('gateway_image_mismatch')
            adapter = SimpleNamespace(current=lambda: previous, activate=activate, healthy=lambda: True)
            gateway = pc.GoogleAnywhere.__new__(pc.GoogleAnywhere)
            gateway.p, gateway.service = object(), 'fixture'
            with patch.object(pc, 'cloud_run_gateway', return_value=adapter):
                with self.assertRaises(pc.InstallError) as failure:
                    gateway.activate_gateway('registry.example/gateway@sha256:' + '1' * 64)
            pc_result = failure.exception.gateway_failure
            def mismatch(image):
                if image != 'old-gateway': raise GatewayError('gateway_image_mismatch')
                self.host.gateway = image
            self.host.activate_gateway = mismatch
            with self.assertRaises(UpdateError): self.install()
            worker_result = {key: self.host.events[-1][key] for key in ('code', 'rolledBack', 'retryable')}
            source = """import {activatePinnedRevision} from './scripts/momso_anywhere_cloud_run_revision.mjs';
let s={etag:'x',invokerIamDisabled:true,terminalCondition:{state:'CONDITION_SUCCEEDED'},latestCreatedRevision:'old-rev',trafficStatuses:[{revision:'old-rev',percent:100}]};
try { await activatePinnedRevision({image:'registry.example/gateway@sha256:'+'1'.repeat(64),service:s,template:{},read:async()=>s,revision:async()=>({conditions:[{type:'Ready',state:'CONDITION_SUCCEEDED'}],containers:[{image:'wrong'}]}),probe:async()=>true,patch:async body=>{s={...s,...body,latestCreatedRevision:'new-rev',trafficStatuses:body.traffic??s.trafficStatuses}}}); }
catch(e){console.log(JSON.stringify({code:e.code,rolledBack:e.rolledBack,retryable:e.retryable}));}"""
            process = subprocess.run(['node', '--input-type=module', '-e', source], cwd=root, capture_output=True, text=True, check=True)
            self.assertEqual(pc_result, worker_result)
            self.assertEqual(pc_result, json.loads(process.stdout))
        finally:
            sys.path.remove(str(root / 'scripts'))

    def test_failed_gateway_rollback_does_not_leave_server_stopped(self):
        running = []
        self.host.start_server = lambda: running.append(self.host.release)
        def deny(image):
            self.host.gateway = "changed"
            raise GatewayError("gateway_operation_failed")
        self.host.activate_gateway = deny
        with self.assertRaisesRegex(UpdateError, "gateway_rollback_failed"): self.install()
        self.assertEqual(self.host.events[-1]['rolledBack'], False)
        self.assertEqual(running[-1], "old")
        self.assertEqual(json.loads(self.updater.state.read_text())["phase"], "switching")

    def test_recovery_of_switching_record_restarts_old_release_and_preserves_snapshot(self):
        self.host.release = "old"; self.host.snapshot = "before"; self.host.data = "partial"
        started = []
        self.host.start_server = lambda: started.append(self.host.release)
        _atomic_json(self.updater.state, {"phase":"switching", "oldRelease":"old", "oldGateway":"old-gateway", "snapshot":"snapshot-1", "newRelease":"new", "sequence":2, "oldSequence":1})
        self.assertEqual(self.updater.recover(), "rolled_back")
        self.assertEqual(started, ["old"])
        self.assertEqual(self.host.snapshot, "before")
        self.assertEqual(self.install(), "completed")

    def test_preflight_permission_error_does_not_stop_server(self):
        self.host.preflight_gateway = lambda _: (_ for _ in ()).throw(GatewayError("gateway_image_permission"))
        self.host.stop_server = lambda: self.fail("server must remain running")
        with self.assertRaisesRegex(UpdateError, "gateway_image_permission"): self.install()
        self.assertFalse(self.host.lease)

    def test_journal_reports_only_safe_codes(self):
        captured = io.StringIO()
        with patch("sys.stdout", captured):
            self.updater._emit("failed", "gateway_image_permission")
            self.updater._emit("failed", "private value /secret")
        self.assertEqual(captured.getvalue(), "update stage=failed reason=gateway_image_permission\n")

    def test_invalid_recovery_record_still_starts_and_checks_server(self):
        self.updater.state.write_text('{broken')
        self.host.start_server = Mock()
        self.host.server_healthy = Mock(return_value=True)
        with self.assertRaisesRegex(UpdateError, "recovery_record_invalid"): self.updater.recover()
        self.host.start_server.assert_called_once_with()
        self.host.server_healthy.assert_called_once_with()
        self.assertEqual(self.updater.state.read_text(), '{broken')

    def test_failed_release_is_retried_without_restoring_data_again(self):
        self.host.healthy = False
        self.host.release_maintenance = Mock(side_effect=RuntimeError("temporary"))
        with self.assertRaisesRegex(UpdateError, "maintenance_release_failed"): self.install()
        record = json.loads(self.updater.state.read_text())
        self.assertEqual(record["phase"], "rolled_back")
        self.assertTrue(record["needsRelease"])
        self.host.data = "new local work"
        self.host.release_maintenance = Mock()
        self.assertEqual(self.updater.recover(), "released")
        self.assertEqual(self.host.data, "new local work")
        self.assertFalse(json.loads(self.updater.state.read_text())["needsRelease"])


if __name__ == "__main__": unittest.main()
