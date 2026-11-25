import hashlib
import json
from pathlib import Path
import tempfile
import unittest
import subprocess
import sys

import managed_helpers
import momso_managed_files as managed


class ManagedHelperTests(unittest.TestCase):
    def setUp(self):
        self.tmp = tempfile.TemporaryDirectory(); self.addCleanup(self.tmp.cleanup)
        self.root = Path(self.tmp.name); self.owner = '0123456789ab'
        self.flat = self.root / 'flat'; self.worker = self.root / 'worker'
        self.private = self.root / 'private'; self.units = self.root / 'units'
        for path in (self.flat, self.worker, self.private, self.units): path.mkdir(mode=0o700)
        self.mgr = managed_helpers.ManagedHelpers(self.flat, self.worker, self.private, self.units)
        self.initial = {'idle-stop.py': b'old idle', 'vault-provision.py': b'old vault'}
        self.old_worker = {'worker.py': b'old worker'}
        self.idle_unit = b'old idle unit'
        for (root, manifest, journal), files, mode in zip(self.mgr._groups(), (self.initial, {'momso-idle-stop.service': self.idle_unit}, self.old_worker), (0o755, 0o644, 0o644)):
            managed.install(root, manifest, journal, self.owner, files, {}, mode)
            journal.unlink()

    def release(self, name, *, idle=b'new idle', worker=b'worker one', unit=b'new idle unit'):
        stage = self.root / name; stage.mkdir(); (stage / 'momso-updater').mkdir(); (stage / 'momso-units').mkdir()
        (stage / 'idle-stop.py').write_bytes(idle); (stage / 'vault-provision.py').write_bytes(b'old vault')
        (stage / 'momso-units/momso-idle-stop.service').write_bytes(unit)
        workers = {name: (worker if name == 'worker.py' else ('fixture ' + name).encode()) for name in managed_helpers.WORKER}
        for name, data in workers.items(): (stage / 'momso-updater' / name).write_bytes(data)
        history = {'version': 1, 'files': {name: [hashlib.sha256(data).hexdigest()] for name, data in {'idle-stop.py': idle, 'vault-provision.py': b'old vault'}.items()},
                   'worker': {name: [hashlib.sha256(data).hexdigest()] for name, data in workers.items()}}
        (stage / 'momso-managed-history.json').write_text(json.dumps(history))
        return stage

    def test_helper_only_update_keeps_worker_and_can_roll_back(self):
        self.mgr.apply(self.release('first')); self.mgr.commit()
        pointer = self.mgr.pointer.readlink()
        self.assertFalse(self.mgr.apply(self.release('second', idle=b'newer idle')))
        self.assertEqual(self.mgr.pointer.readlink(), pointer)
        self.assertEqual((self.flat / 'idle-stop.py').read_bytes(), b'newer idle')
        self.mgr.rollback()
        self.assertEqual((self.flat / 'idle-stop.py').read_bytes(), b'new idle')
        self.assertEqual(self.mgr.pointer.readlink(), pointer)

    def test_worker_only_update_uses_new_immutable_version_and_rolls_back(self):
        self.mgr.apply(self.release('first', worker=b"print('worker-one')")); self.mgr.commit()
        pointer = self.mgr.pointer.readlink()
        self.assertTrue(self.mgr.apply(self.release('second', worker=b"print('worker-two')")))
        self.assertNotEqual(self.mgr.pointer.readlink(), pointer)
        self.assertEqual((self.worker / 'worker.py').read_bytes(), managed_helpers.LAUNCHER)
        self.assertEqual(subprocess.check_output([sys.executable, str(self.worker / 'worker.py')]).strip(), b'worker-two')
        managed_helpers.ManagedHelpers(self.flat, self.worker, self.private, self.units).rollback()
        self.assertEqual(self.mgr.pointer.readlink(), pointer)
        self.assertEqual(subprocess.check_output([sys.executable, str(self.worker / 'worker.py')]).strip(), b'worker-one')
        self.assertEqual((self.flat / 'idle-stop.py').read_bytes(), b'new idle')

    def test_unrecognized_helper_is_preserved_and_partial_changes_undone(self):
        (self.flat / 'idle-stop.py').write_bytes(b'user edit')
        with self.assertRaises(RuntimeError): self.mgr.apply(self.release('release'))
        self.assertEqual((self.flat / 'idle-stop.py').read_bytes(), b'user edit')
        self.assertEqual((self.worker / 'worker.py').read_bytes(), b'old worker')
        self.assertFalse(self.mgr.pointer.exists())

    def test_tampered_release_is_rejected_before_managed_changes(self):
        stage = self.release('release'); (stage / 'idle-stop.py').write_bytes(b'tampered')
        with self.assertRaisesRegex(ValueError, 'invalid_release_helpers'): self.mgr.apply(stage)
        self.assertEqual((self.flat / 'idle-stop.py').read_bytes(), b'old idle')

    def test_signed_restore_workers_are_installed_together(self):
        from momso_anywhere_restore_files import RESTORE_FILES
        scripts = Path(__file__).resolve().parents[5] / 'scripts'
        stage = self.release('restore')
        history = json.loads((stage / 'momso-managed-history.json').read_text())
        for name in RESTORE_FILES:
            data = (scripts / name).read_bytes()
            (stage / name).write_bytes(data)
            history['files'][name] = [hashlib.sha256(data).hexdigest()]
        for name in ('momso_anywhere_restore_files.py', 'momso_anywhere_restore_files.json'):
            data = (scripts / name).read_bytes()
            (stage / 'momso-updater' / name).write_bytes(data)
            history['worker'][name] = [hashlib.sha256(data).hexdigest()]
        (stage / 'momso-managed-history.json').write_text(json.dumps(history))
        self.mgr.apply(stage)
        self.assertTrue(all((self.flat / name).is_file() for name in RESTORE_FILES))
        if sys.platform != 'win32':
            code = "import sys; sys.path.insert(0,sys.argv[1]); import momso_anywhere_prestart_restore, momso_anywhere_backup_agent"
            subprocess.run([sys.executable, '-I', '-c', code, str(self.flat)], check=True)

    def test_signed_vault_override_updates_without_touching_customer_backup_override(self):
        directory = self.units / 'momso-server.service.d'; directory.mkdir(mode=0o700)
        root, manifest, journal = self.mgr._groups()[3]
        managed.install(root, manifest, journal, self.owner, {'vault.conf': b'old vault', 'backup.conf': b'customer settings'}, {}, 0o644)
        journal.unlink()
        stage = self.release('override'); (stage / 'momso-overrides').mkdir()
        (stage / 'momso-overrides/vault.conf').write_bytes(b'new vault')
        self.mgr.apply(stage)
        self.assertTrue(self.mgr.changed_units)
        self.assertEqual((directory / 'vault.conf').read_bytes(), b'new vault')
        self.assertEqual((directory / 'backup.conf').read_bytes(), b'customer settings')
        self.mgr.rollback()
        self.assertEqual((directory / 'vault.conf').read_bytes(), b'old vault')


if __name__ == '__main__': unittest.main()
