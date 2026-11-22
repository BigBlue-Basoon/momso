"""Exercise managed helper transactions through the signed update entry point."""
import hashlib
import io
import json
import tarfile

import managed_helpers
import momso_managed_files as managed
import test_update
from update import UpdateError


class ManagedSignedUpdateTests(test_update.UpdateTests):
    def setUp(self):
        super().setUp()
        self.flat = self.root / 'flat'; self.worker = self.root / 'worker'
        self.private_dir = self.root / 'private'; self.units = self.root / 'units'
        for path in (self.flat, self.worker, self.private_dir, self.units): path.mkdir(mode=0o700)
        self.manager = managed_helpers.ManagedHelpers(self.flat, self.worker, self.private_dir, self.units)
        self.owner = '0123456789ab'
        for (root, manifest, journal), files, mode in zip(self.manager._groups(),
            ({'idle-stop.py': b'old'}, {'momso-idle-stop.service': b'unit'}, {'worker.py': b'old worker'}),
            (0o755, 0o644, 0o644)):
            managed.install(root, manifest, journal, self.owner, files, {}, mode)
            journal.unlink()
        self.host.apply_managed = self.manager.apply
        self.host.rollback_managed = self.manager.rollback
        self.host.commit_managed = self.manager.commit
        self.running = True
        self.host.stop_server = lambda: setattr(self, 'running', False)
        self.host.start_server = lambda: setattr(self, 'running', True)

    def publish_managed(self, sequence, idle, worker):
        import subprocess
        payload = {name: name.encode() for name in ('momso-server', 'momso_mobile_proxy.py', 'momso_anywhere_vm_proxy.py', 'web/index.html')}
        payload['idle-stop.py'] = idle
        payload['momso-units/momso-idle-stop.service'] = b'unit'
        workers = {name: (worker if name == 'worker.py' else name.encode()) for name in managed_helpers.WORKER}
        payload.update({'momso-updater/' + name: value for name, value in workers.items()})
        history = {'version': 1, 'files': {'idle-stop.py': [hashlib.sha256(idle).hexdigest()]},
                   'worker': {name: [hashlib.sha256(value).hexdigest()] for name, value in workers.items()}}
        payload['momso-managed-history.json'] = json.dumps(history).encode()
        with tarfile.open(self.server_dir / 'bundle.tar.gz', 'w:gz') as archive:
            for name, data in payload.items():
                entry = tarfile.TarInfo(name); entry.size = len(data); archive.addfile(entry, io.BytesIO(data))
        manifest = {'sequence': sequence, 'version': '0.1.2', 'bundleUrl': self.base + '/bundle.tar.gz',
                    'bundleSha256': hashlib.sha256((self.server_dir / 'bundle.tar.gz').read_bytes()).hexdigest(),
                    'gatewayImage': 'registry.example/momso@sha256:' + '1' * 64}
        (self.server_dir / 'manifest.json').write_text(json.dumps(manifest))
        subprocess.run(['openssl', 'pkeyutl', '-sign', '-inkey', str(self.private), '-rawin', '-in', str(self.server_dir / 'manifest.json'), '-out', str(self.server_dir / 'manifest.sig')], check=True, capture_output=True)

    def test_signed_helper_and_worker_updates_and_failed_switch(self):
        self.publish_managed(2, b'idle one', b'worker one')
        self.assertEqual(self.install(), 'completed')
        first_pointer = self.manager.pointer.readlink()
        self.publish_managed(3, b'idle two', b'worker one')
        self.assertEqual(self.install(), 'completed')
        self.assertEqual((self.flat / 'idle-stop.py').read_bytes(), b'idle two')
        self.assertEqual(self.manager.pointer.readlink(), first_pointer)
        self.publish_managed(4, b'idle two', b'worker two')
        self.assertEqual(self.install(), 'completed')
        second_pointer = self.manager.pointer.readlink()
        self.assertNotEqual(first_pointer, second_pointer)
        self.publish_managed(5, b'idle three', b'worker three')
        previous_release = self.host.release
        self.host.server_healthy = lambda: self.host.release == previous_release
        with self.assertRaisesRegex(UpdateError, 'server_unhealthy'):
            self.install()
        self.assertEqual((self.flat / 'idle-stop.py').read_bytes(), b'idle two')
        self.assertEqual(self.manager.pointer.readlink(), second_pointer)
        self.assertEqual(self.host.sequence, 4)
        self.assertTrue(self.running)

