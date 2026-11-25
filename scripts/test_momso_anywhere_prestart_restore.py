import hashlib
import fcntl
import io
import json
from pathlib import Path
import sqlite3
import tempfile
import unittest
from contextlib import redirect_stdout
from types import SimpleNamespace
from unittest.mock import patch

import momso_anywhere_backup as backup
import momso_anywhere_prestart_restore as worker
from momso_anywhere_prestart_restore import RestoreFailure, protect_first_backup, restore_before_start, services_stopped, verify_cloud_ownership
from test_momso_anywhere_backup import FakeKeys, FileStore


class Store(FileStore):
    def bucket_retention(self): return {'softDeleteSeconds': 0}


class PrestartRestoreTests(unittest.TestCase):
    def setUp(self):
        self.temp = tempfile.TemporaryDirectory()
        self.addCleanup(self.temp.cleanup)
        self.root = Path(self.temp.name)
        self.source = self.root / 'original'
        self.source.mkdir()
        self.remote = Store(self.root / 'remote')
        self.keys = FakeKeys()
        self.destination = self.root / 'new-vm'
        self.identity = self.root / 'vault-identity'
        self.identity.write_bytes(b'fixture-identity')
        self.started = False
        self.backup_started = False

    def startup(self):
        result = restore_before_start(self.destination, self.remote, self.keys, self.identity, stopped=lambda: not self.started)
        self.started = True
        self.backup_started = True
        return result

    def seed_backup(self):
        with sqlite3.connect(self.source / 'server.sqlite3') as db:
            db.execute('create table marker(value text not null)')
            db.execute('insert into marker values (?)', ('fixture lesson',))
        (self.source / 'archive').mkdir()
        self.assertEqual(backup.backup(self.source, self.remote, self.keys, ['server.sqlite3'], archive=self.source / 'archive', quiescent=True), 'uploaded')

    def remote_bytes(self):
        return {p.name: p.read_bytes() for p in self.remote.root.iterdir() if p.is_file()}

    def test_no_backup_starts_without_restore(self):
        result = self.startup()
        self.assertEqual(result, {'restored': False, 'restoredBackupId': None, 'restoredAt': None})
        self.assertTrue(self.started)
        self.assertTrue(self.backup_started)
        self.assertFalse((self.destination / '.momso-restore-pending-first-backup').exists())

    def test_real_files_restore_before_start_without_pruning(self):
        self.seed_backup()
        before = self.remote_bytes()
        result = self.startup()
        self.assertTrue(result['restored'])
        self.assertRegex(result['restoredBackupId'], r'^slot-\d@[0-9]+$')
        self.assertTrue(result['restoredAt'])
        self.assertTrue(self.started)
        self.assertTrue(self.backup_started)
        self.assertTrue((self.destination / '.momso-restore-pending-first-backup').is_file())
        def marker_hash(path):
            with sqlite3.connect(path) as db:
                return hashlib.sha256(db.execute('select value from marker').fetchone()[0].encode()).digest()
        self.assertEqual(marker_hash(self.source / 'server.sqlite3'), marker_hash(self.destination / 'server.sqlite3'))
        self.assertEqual(self.remote_bytes(), before)

    def test_bad_key_never_starts_and_retained_backup_is_unchanged(self):
        self.seed_backup()
        before = self.remote_bytes()
        self.keys = FakeKeys()
        with self.assertRaises(RestoreFailure) as caught:
            self.startup()
        self.assertEqual(caught.exception.detail, 'key_unavailable')
        self.assertFalse(self.started)
        self.assertFalse(self.backup_started)
        self.assertEqual(self.remote_bytes(), before)

    def test_malformed_unwrapped_key_never_starts(self):
        self.seed_backup()
        before = self.remote_bytes()
        class BadKeys:
            def unwrap(self, _value): return b'bad'
        self.keys = BadKeys()
        with self.assertRaises(RestoreFailure) as caught:
            self.startup()
        self.assertEqual(caught.exception.detail, 'key_unavailable')
        self.assertFalse(self.started)
        self.assertEqual(self.remote_bytes(), before)

    def test_latest_backup_is_used_when_multiple_slots_exist(self):
        self.seed_backup()
        with sqlite3.connect(self.source / 'server.sqlite3') as db:
            db.execute('update marker set value = ?', ('latest fixture',))
        self.assertEqual(backup.backup(self.source, self.remote, self.keys, ['server.sqlite3'], archive=self.source / 'archive', quiescent=True), 'uploaded')
        before = self.remote_bytes()
        result = self.startup()
        self.assertTrue(result['restored'])
        self.assertTrue(result['restoredBackupId'].startswith('slot-1@'))
        with sqlite3.connect(self.destination / 'server.sqlite3') as db:
            self.assertEqual(db.execute('select value from marker').fetchone()[0], 'latest fixture')
        self.assertEqual(self.remote_bytes(), before)

    def test_all_old_slots_remain_before_first_new_backup(self):
        self.seed_backup()
        for index in range(1, backup.SLOT_COUNT):
            with sqlite3.connect(self.source / 'server.sqlite3') as db:
                db.execute('update marker set value = ?', (f'fixture {index}',))
            self.assertEqual(backup.backup(self.source, self.remote, self.keys, ['server.sqlite3'], archive=self.source / 'archive', quiescent=True), 'uploaded')
        before = self.remote_bytes()
        self.assertEqual(sum(name.startswith('slot-') and not name.endswith('.generation') for name in before), backup.SLOT_COUNT)
        restored = self.startup()
        self.assertTrue(restored['restored'])
        self.assertEqual(self.remote_bytes(), before)
        with sqlite3.connect(self.destination / 'server.sqlite3') as db:
            db.execute('update marker set value = ?', ('new first backup',))
        source_slot = int(restored['restoredBackupId'].split('@')[0].split('-')[1])
        original = self.remote.read_versioned(source_slot)
        self.assertEqual(backup.backup(self.destination, self.remote, self.keys, ['server.sqlite3'],
                                       archive=self.destination / 'archive', quiescent=True,
                                       preserve_existing_slots=True, protected_slot=source_slot), 'uploaded')
        self.assertEqual(self.remote.read_versioned(source_slot), original)
        self.assertEqual(sum(self.remote.read_versioned(i) is not None for i in range(8)), 7)
        self.assertTrue((self.destination / '.momso-restore-pending-first-backup').is_file())

    def fill_seven(self):
        self.seed_backup()
        for index in range(1, 7):
            with sqlite3.connect(self.source / 'server.sqlite3') as db:
                db.execute('update marker set value = ?', (f'fixture {index}',))
            backup.backup(self.source, self.remote, self.keys, ['server.sqlite3'],
                          archive=self.source / 'archive', quiescent=True)

    def test_failed_verification_preserves_all_seven(self):
        self.fill_seven()
        before = [self.remote.read_versioned(i) for i in range(7)]
        with sqlite3.connect(self.source / 'server.sqlite3') as db:
            db.execute('update marker set value = ?', ('changed',))
        with patch.object(backup, 'verify_backup_blob', side_effect=ValueError('corrupt')):
            with self.assertRaisesRegex(ValueError, 'corrupt'):
                backup.backup(self.source, self.remote, self.keys, ['server.sqlite3'],
                              archive=self.source / 'archive', quiescent=True)
        self.assertEqual([self.remote.read_versioned(i) for i in range(7)], before)
        self.assertIsNone(self.remote.read_versioned(7))

    def test_bad_interrupted_temporary_slot_is_removed_with_fewer_regular_slots(self):
        self.seed_backup()
        original = self.remote.read_versioned(0)
        self.remote.create_slot_if_absent(7, b'invalid staged backup')
        with self.assertRaises(ValueError):
            backup._finish_staged_backup(self.remote, self.keys, ['server.sqlite3'])
        self.assertEqual(self.remote.read_versioned(0), original)
        self.assertIsNone(self.remote.read_versioned(7))

    def test_oldest_restored_backup_is_not_removed(self):
        self.fill_seven()
        protected = self.remote.read_versioned(0)
        with sqlite3.connect(self.source / 'server.sqlite3') as db:
            db.execute('update marker set value = ?', ('changed',))
        backup.backup(self.source, self.remote, self.keys, ['server.sqlite3'],
                      archive=self.source / 'archive', quiescent=True,
                      preserve_existing_slots=True, protected_slot=0)
        self.assertEqual(self.remote.read_versioned(0), protected)
        self.assertEqual(sum(self.remote.read_versioned(i) is not None for i in range(8)), 7)

    def test_interruption_after_atomic_replace_recovers_and_never_exceeds_eight(self):
        self.fill_seven()
        with sqlite3.connect(self.source / 'server.sqlite3') as db:
            db.execute('update marker set value = ?', ('changed',))
        create = self.remote.create_slot_if_absent
        replace = self.remote.replace_if_generation
        maximum = [0]
        def interrupt(slot, blob):
            create(slot, blob)
            maximum[0] = max(maximum[0], sum(self.remote.read_versioned(i) is not None for i in range(8)))
        def interrupt_after_replace(slot, generation, blob):
            replace(slot, generation, blob)
            raise OSError('simulated crash')
        with patch.object(self.remote, 'create_slot_if_absent', side_effect=interrupt), \
             patch.object(self.remote, 'replace_if_generation', side_effect=interrupt_after_replace):
            with self.assertRaisesRegex(OSError, 'simulated crash'):
                backup.backup(self.source, self.remote, self.keys, ['server.sqlite3'],
                              archive=self.source / 'archive', quiescent=True)
        self.assertIsNotNone(self.remote.read_versioned(7))
        self.assertEqual(sum(self.remote.read_versioned(i) is not None for i in range(8)), 8)
        backup.backup(self.source, self.remote, self.keys, ['server.sqlite3'],
                      archive=self.source / 'archive', quiescent=True)
        self.assertIsNone(self.remote.read_versioned(7))
        self.assertEqual(sum(self.remote.read_versioned(i) is not None for i in range(8)), 7)
        self.assertEqual(maximum[0], 8)

    def test_restore_uses_temporary_only_when_every_regular_slot_is_invalid(self):
        self.seed_backup()
        blob = self.remote.read(0)
        self.remote.create_slot_if_absent(7, blob)
        self.assertTrue(self.startup()['restoredBackupId'].startswith('slot-0@'))
        self.started = False
        self.destination = self.root / 'fallback-vm'
        self.remote.write(0, b'invalid')
        result = self.startup()
        self.assertTrue(result['restoredFromTemporary'])
        self.assertIsNone(self.remote.read_versioned(7))
        self.assertRegex(result['restoredBackupId'], r'^slot-[0-6]@[0-9]+$')

    def test_corrupt_backup_never_starts_or_removes_retained_data(self):
        self.seed_backup()
        chunks = list(self.remote.root.glob('chunk-*'))
        self.assertTrue(chunks)
        data = bytearray(chunks[0].read_bytes())
        data[-1] ^= 1
        chunks[0].write_bytes(data)
        before = self.remote_bytes()
        with self.assertRaises(RestoreFailure) as caught:
            self.startup()
        self.assertEqual(caught.exception.detail, 'checksum_mismatch')
        self.assertFalse(self.started)
        self.assertFalse(self.backup_started)
        self.assertEqual(self.remote_bytes(), before)

    def test_vault_identity_must_match_preserved_secret(self):
        vault = self.source / 'saved-logins' / 'vault.age'
        vault.parent.mkdir()
        vault.write_bytes(b'fixture encrypted vault')
        (self.source / 'archive').mkdir()
        backup.backup(self.source, self.remote, self.keys, ['saved-logins/vault.age'], vault_identity=b'fixture-identity', archive=self.source / 'archive', quiescent=True)
        self.identity.write_bytes(b'different-identity')
        before = self.remote_bytes()
        with self.assertRaises(RestoreFailure) as caught:
            self.startup()
        self.assertEqual(caught.exception.detail, 'key_unavailable')
        self.assertFalse(self.started)
        self.assertEqual(self.remote_bytes(), before)

    def test_existing_different_database_is_never_overwritten(self):
        self.seed_backup()
        self.destination.mkdir()
        existing = self.destination / 'server.sqlite3'
        existing.write_bytes(b'local data that must remain')
        before = self.remote_bytes()
        with self.assertRaises(RestoreFailure) as caught:
            self.startup()
        self.assertEqual(caught.exception.detail, 'restore_conflict')
        self.assertEqual(existing.read_bytes(), b'local data that must remain')
        self.assertFalse(self.started)
        self.assertEqual(self.remote_bytes(), before)

    def test_backup_changed_during_restore_never_starts_server(self):
        self.seed_backup()
        original = self.remote.read_versioned
        calls = []
        def changed(slot):
            calls.append(slot)
            version, data = original(slot)
            return (version if len(calls) == 1 else str(int(version) + 1), data)
        self.remote.read_versioned = changed
        with self.assertRaises(RestoreFailure) as caught:
            self.startup()
        self.assertEqual(caught.exception.detail, 'backup_changed')
        self.assertFalse(self.started)

    def test_backup_disappearing_during_restore_is_not_an_empty_install(self):
        self.seed_backup()
        before = self.remote_bytes()
        with patch.object(backup, 'restore', return_value={'status': 'no_backup', 'conflicts': []}):
            with self.assertRaises(RestoreFailure) as caught:
                self.startup()
        self.assertEqual(caught.exception.detail, 'backup_changed')
        self.assertFalse(self.started)
        self.assertEqual(self.remote_bytes(), before)

    def test_activating_service_is_not_treated_as_stopped(self):
        class Result:
            returncode = 0
            stdout = b'activating\n'
        self.assertFalse(services_stopped(run=lambda *_args, **_kwargs: Result()))

    def test_active_backup_timer_blocks_restore(self):
        class Result:
            returncode = 0
            def __init__(self, state): self.stdout = state
        def query(command, **_kwargs):
            return Result(b'active\n' if 'momso-backup.timer' in command else b'inactive\n')
        self.assertFalse(services_stopped(run=query))

    def test_running_service_never_restores(self):
        self.started = True
        with self.assertRaises(RestoreFailure) as caught:
            restore_before_start(self.destination, self.remote, self.keys, self.identity, stopped=lambda: not self.started)
        self.assertEqual(caught.exception.detail, 'services_not_stopped')

    def test_unreadable_service_state_never_restores(self):
        def unknown(): raise OSError('fixture unavailable')
        with self.assertRaises(RestoreFailure) as caught:
            restore_before_start(self.destination, self.remote, self.keys, self.identity, stopped=unknown)
        self.assertEqual(caught.exception.detail, 'services_not_stopped')
        self.assertFalse(self.destination.exists())

    def test_unreachable_bucket_is_not_mistaken_for_empty_backup(self):
        class Unreachable(Store):
            def bucket_retention(self): raise OSError('fixture unavailable')
        self.remote = Unreachable(self.root / 'unreachable')
        with self.assertRaises(RestoreFailure) as caught:
            self.startup()
        self.assertEqual(caught.exception.detail, 'backup_unreadable')
        self.assertFalse(self.started)

    def test_active_backup_lock_blocks_restore_without_waiting(self):
        runtime = self.destination / '.backup-runtime'
        runtime.mkdir(parents=True)
        with (runtime / 'lock').open('a+b') as lock:
            fcntl.flock(lock, fcntl.LOCK_EX)
            with self.assertRaises(RestoreFailure) as caught:
                self.startup()
        self.assertEqual(caught.exception.detail, 'backup_busy')
        self.assertFalse(self.started)
        self.assertFalse(self.backup_started)

    def test_restore_marker_is_atomic_idempotent_and_never_replaces_foreign_file(self):
        self.destination.mkdir()
        protect_first_backup(self.destination)
        protect_first_backup(self.destination)
        marker = self.destination / '.momso-restore-pending-first-backup'
        self.assertEqual(marker.read_bytes(), b'momso-restore-v1\n')
        self.assertFalse(list(self.destination.glob('.momso-restore-marker-*')))
        marker.write_bytes(b'foreign fixture')
        with self.assertRaises(RestoreFailure): protect_first_backup(self.destination)
        self.assertEqual(marker.read_bytes(), b'foreign fixture')

    def test_interrupted_marker_install_does_not_leave_partial_marker(self):
        self.destination.mkdir()
        with patch.object(worker.os, 'link', side_effect=OSError('fixture interruption')):
            with self.assertRaises(OSError): protect_first_backup(self.destination)
        self.assertFalse((self.destination / '.momso-restore-pending-first-backup').exists())
        self.assertFalse(list(self.destination.glob('.momso-restore-marker-*')))
        protect_first_backup(self.destination)
        self.assertTrue((self.destination / '.momso-restore-pending-first-backup').is_file())

    def test_backup_appearing_during_empty_check_blocks_start(self):
        calls = []
        def modified(slot):
            calls.append(slot)
            return 1 if len(calls) > backup.SLOT_COUNT and slot == 0 else None
        self.remote.modified = modified
        with self.assertRaises(RestoreFailure) as caught:
            self.startup()
        self.assertEqual(caught.exception.detail, 'backup_changed')
        self.assertFalse(self.started)
        self.assertFalse(self.backup_started)

    def test_foreign_cloud_backup_is_rejected_before_restore(self):
        installation = 'a' * 12
        bucket = 'momso-backup-' + installation
        key = 'projects/fixture-project/locations/asia-northeast3/keyRings/' + bucket + '/cryptoKeys/backup'
        class Client:
            def request(self, url):
                return json.dumps({'name': bucket if 'storage.googleapis.com' in url else key,
                                   'labels': {'momso-install-id': 'b' * 12}}).encode()
        with self.assertRaises(RestoreFailure) as caught:
            verify_cloud_ownership(Client(), bucket, key, installation)
        self.assertEqual(caught.exception.detail, 'ownership_unverified')

    def test_owned_cloud_backup_requires_matching_full_installation_id(self):
        installation = 'a' * 24
        bucket = 'momso-backup-' + installation[:12]
        key = 'projects/fixture-project/locations/asia-northeast3/keyRings/' + bucket + '/cryptoKeys/backup'
        class Client:
            def request(self, url):
                return json.dumps({'name': bucket if 'storage.googleapis.com' in url else key,
                                   'labels': {'momso-install-id': installation}}).encode()
        verify_cloud_ownership(Client(), bucket, key, installation)
        with self.assertRaises(RestoreFailure):
            verify_cloud_ownership(Client(), bucket, key, installation[:12])

    def test_inaccessible_key_reports_key_unavailable(self):
        installation = 'a' * 12
        bucket = 'momso-backup-' + installation
        key = 'projects/fixture-project/locations/asia-northeast3/keyRings/' + bucket + '/cryptoKeys/backup'
        class Client:
            def request(self, url):
                if 'cloudkms' in url: raise OSError('fixture secret')
                return json.dumps({'name': bucket, 'labels': {'momso-install-id': installation}}).encode()
        with self.assertRaises(RestoreFailure) as caught:
            verify_cloud_ownership(Client(), bucket, key, installation)
        self.assertEqual(caught.exception.detail, 'key_unavailable')
        self.assertNotIn('fixture secret', str(caught.exception))

    def test_cli_returns_failure_and_detail_without_starting_services(self):
        args = ['--installation-id', 'a' * 12, '--data-dir', '/var/lib/momso',
                '--bucket', 'fixture', '--kms-key', 'fixture', '--vault-identity',
                '/run/credentials/momso-restore.service/momso-vault-identity',
                '--report', '/var/lib/momso/.momso-prestart-restore.json']
        reports = []
        with patch.object(worker.os, 'geteuid', return_value=123), patch.object(worker.pwd, 'getpwnam', return_value=SimpleNamespace(pw_uid=123)), patch.object(worker, 'GoogleClient'), patch.object(worker, 'CloudStorage'), patch.object(worker, 'CloudKms'), patch.object(worker, 'verify_cloud_ownership'), patch.object(worker, 'restore_before_start', side_effect=RestoreFailure('key_unavailable')), patch.object(worker, 'write_report', side_effect=lambda _path, value: reports.append(value)), redirect_stdout(io.StringIO()) as output:
            self.assertEqual(worker.main(args), 1)
        self.assertEqual(json.loads(output.getvalue()), {'ok': False, 'code': 'restore_failed', 'detail': 'key_unavailable'})
        self.assertEqual(reports[0]['detail'], 'key_unavailable')

    def test_cli_returns_restore_metadata_to_installer(self):
        args = ['--installation-id', 'a' * 12, '--data-dir', '/var/lib/momso',
                '--bucket', 'fixture', '--kms-key', 'fixture', '--vault-identity',
                '/run/credentials/momso-restore.service/momso-vault-identity',
                '--report', '/var/lib/momso/.momso-prestart-restore.json']
        value = {'restored': True, 'restoredBackupId': 'slot-1@123', 'restoredAt': '2026-01-01T00:00:00Z'}
        reports = []
        with patch.object(worker.os, 'geteuid', return_value=123), patch.object(worker.pwd, 'getpwnam', return_value=SimpleNamespace(pw_uid=123)), patch.object(worker, 'GoogleClient'), patch.object(worker, 'CloudStorage'), patch.object(worker, 'CloudKms'), patch.object(worker, 'verify_cloud_ownership'), patch.object(worker, 'restore_before_start', return_value=value), patch.object(worker, 'write_report', side_effect=lambda _path, report: reports.append(report)), redirect_stdout(io.StringIO()) as output:
            self.assertEqual(worker.main(args), 0)
        self.assertEqual(json.loads(output.getvalue()), {'ok': True, **value})
        self.assertEqual(reports[0], {'ok': True, **value})


if __name__ == '__main__': unittest.main()
