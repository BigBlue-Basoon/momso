import hashlib
import json
import os
from pathlib import Path
import tempfile
import unittest

import momso_managed_files as managed


@unittest.skipIf(os.name == 'nt', 'managed VM files require Linux filesystem operations')
class ManagedFilesTests(unittest.TestCase):
    def setUp(self):
        self.tmp = tempfile.TemporaryDirectory()
        self.addCleanup(self.tmp.cleanup)
        self.root = Path(self.tmp.name)
        self.manifest = self.root / '.manifest'
        self.journal = self.root / '.journal'
        self.owner = 'fixture-owner'

    def install(self, files, legacy=None):
        managed.install(self.root, self.manifest, self.journal, self.owner, files, legacy or {})

    def rollback(self):
        managed.rollback(self.root, self.manifest, self.journal, self.owner)

    def test_legacy_release_is_replaced_recorded_and_restored(self):
        old, new = b'old release', b'new release'
        (self.root / 'worker.py').write_bytes(old)
        digest = hashlib.sha256(old).hexdigest()
        self.install({'worker.py': new}, {'worker.py': [digest]})
        self.assertEqual((self.root / 'worker.py').read_bytes(), new)
        self.assertEqual(json.loads(self.manifest.read_text())['files']['worker.py'], hashlib.sha256(new).hexdigest())
        self.rollback()
        self.assertEqual((self.root / 'worker.py').read_bytes(), old)
        self.assertFalse(self.manifest.exists())

    def test_manifest_allows_subsequent_upgrade_and_restores_prior_manifest(self):
        self.install({'worker.py': b'v1'})
        self.journal.unlink()
        before = self.manifest.read_bytes()
        self.install({'worker.py': b'v2'})
        self.rollback()
        self.assertEqual((self.root / 'worker.py').read_bytes(), b'v1')
        self.assertEqual(self.manifest.read_bytes(), before)

    def test_foreign_file_rejects_before_any_write(self):
        (self.root / 'foreign.py').write_bytes(b'foreign')
        with self.assertRaisesRegex(RuntimeError, 'momso-managed-conflict: foreign.py: unrecognized'):
            self.install({'first.py': b'new', 'foreign.py': b'replacement'})
        self.assertFalse((self.root / 'first.py').exists())
        self.assertEqual((self.root / 'foreign.py').read_bytes(), b'foreign')
        self.assertFalse(self.journal.exists())

    def test_tampering_blocks_rollback_without_overwriting_it(self):
        self.install({'worker.py': b'new'})
        (self.root / 'worker.py').write_bytes(b'foreign edit')
        with self.assertRaisesRegex(RuntimeError, 'managed_rollback_conflict'):
            self.rollback()
        self.assertEqual((self.root / 'worker.py').read_bytes(), b'foreign edit')
        self.assertTrue(self.journal.exists())

    def test_interrupted_install_is_rolled_back_before_retry(self):
        self.install({'worker.py': b'v1'})
        self.install({'worker.py': b'v2'})
        self.assertEqual((self.root / 'worker.py').read_bytes(), b'v2')
        self.rollback()
        self.assertFalse((self.root / 'worker.py').exists())

    def test_writable_management_directory_is_rejected(self):
        self.root.chmod(0o777)
        self.addCleanup(self.root.chmod, 0o700)
        with self.assertRaisesRegex(RuntimeError, 'managed_directory_conflict'):
            self.install({'worker.py': b'new'})
        self.assertFalse((self.root / 'worker.py').exists())

    def test_first_migration_adopts_only_explicit_generated_names(self):
        (self.root / 'idle-token.py').write_bytes(b'old generated content')
        self.install_adopt = lambda: managed.install(self.root, self.manifest, self.journal, self.owner, {'idle-token.py': b'new'}, {}, adopt=('idle-token.py',), first_migration=True)
        self.install_adopt()
        self.assertEqual((self.root / 'idle-token.py').read_bytes(), b'new')
        self.assertEqual(json.loads(self.manifest.read_text())['files']['idle-token.py'], hashlib.sha256(b'new').hexdigest())
        self.journal.unlink()
        (self.root / 'idle-token.py').write_bytes(b'changed later')
        with self.assertRaisesRegex(RuntimeError, 'idle-token.py: unrecognized'):
            self.install_adopt()

    def test_scan_reports_all_conflicts_without_writes(self):
        (self.root / 'first.py').write_bytes(b'foreign one')
        (self.root / 'second.py').write_bytes(b'foreign two')
        issues = managed.conflicts(self.root, self.manifest, self.owner, {'first.py': b'new', 'second.py': b'new'}, {})
        self.assertEqual(issues, [('first.py', 'unrecognized'), ('second.py', 'unrecognized')])
        self.assertEqual((self.root / 'first.py').read_bytes(), b'foreign one')
        self.assertFalse(self.manifest.exists())

    def test_invalid_existing_ledger_does_not_enable_first_adoption(self):
        self.manifest.write_bytes(b'invalid fixture')
        (self.root / 'idle-token.py').write_bytes(b'unknown')
        issues = managed.conflicts(self.root, self.manifest, self.owner, {'idle-token.py': b'new'}, {}, adopt=('idle-token.py',), first_migration=True)
        self.assertEqual(issues, [('.manifest', 'manifest_invalid'), ('idle-token.py', 'unrecognized')])
        self.assertEqual((self.root / 'idle-token.py').read_bytes(), b'unknown')

    def test_first_migration_never_adopts_a_symlink(self):
        target = self.root / 'private-data'; target.write_bytes(b'keep')
        (self.root / 'idle-token.py').symlink_to(target)
        issues = managed.conflicts(self.root, self.manifest, self.owner, {'idle-token.py': b'new'}, {}, adopt=('idle-token.py',), first_migration=True)
        self.assertEqual(issues, [('idle-token.py', 'not_regular')])
        self.assertEqual(target.read_bytes(), b'keep')


if __name__ == '__main__':
    unittest.main()
