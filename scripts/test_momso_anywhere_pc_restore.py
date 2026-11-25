import ast
import base64
import io
import json
import re
import tarfile
import unittest
from unittest.mock import patch

import momso_anywhere_pc_restore as restore
from momso_anywhere_restore_files import RESTORE_FILES
from momso_server_install import InstallError
from momso_anywhere_vm_resources import names


def bundle():
    value = io.BytesIO()
    with tarfile.open(fileobj=value, mode='w:gz') as archive:
        for name in (*RESTORE_FILES, 'vault-provision.py'):
            info = tarfile.TarInfo(name); info.size = 1
            archive.addfile(info, io.BytesIO(b'x'))
    return value.getvalue()


class Tests(unittest.TestCase):
    def test_existing_vm_goes_to_upgrade_only_with_owned_gateway(self):
        class Provider:
            install_id = 'a' * 12
            name = 'momso-' + install_id
            def run(self, *args, **_kwargs):
                if args[:3] == ('compute', 'instances', 'list'):
                    return [{'name': self.name, 'labels': {'momso-install-id': self.install_id}}]
                return {'status': {'url': 'https://fixture.a.run.app'}}
        provider = Provider()
        with patch('momso_anywhere_install.GoogleAnywhere') as gateway:
            gateway.return_value._objects.return_value = ({}, {}, {})
            with self.assertRaisesRegex(InstallError, 'restore_vm_exists'):
                restore.existing_owned_vm_can_upgrade(provider, {'url': 'https://fixture.a.run.app'})
            gateway.return_value._objects.return_value = ({'owned': True}, {'owned': True}, {'owned': True})
            self.assertTrue(restore.existing_owned_vm_can_upgrade(provider, {'url': 'https://fixture.a.run.app'}))
            with self.assertRaisesRegex(InstallError, 'restore_vm_exists'):
                restore.existing_owned_vm_can_upgrade(provider, {'url': 'https://other.a.run.app'})

    def test_identity_rebinds_without_creating_preserved_assets(self):
        class Provider:
            install_id = 'a' * 12
            name = 'momso-' + install_id
            project = 'fixture-project'
            def __init__(self): self.calls = []
            def run(self, *args, **kwargs):
                self.calls.append(args)
                if args[:3] == ('compute', 'instances', 'describe'):
                    return {'labels': {'momso-install-id': self.install_id}, 'serviceAccounts': []}
                return b''
        p = Provider()
        with patch('momso_anywhere_vm_services.identity', return_value='fixture-account'), patch('momso_anywhere_vm_services.secret', return_value=('fixture-secret', False)) as secret, patch('momso_anywhere_backup_provision.GoogleBackupProvisioner') as provisioner:
            provisioner.return_value.provision.return_value = {'bucket': 'fixture', 'key': 'fixture'}
            restore.prepare_identity(p)
        provisioner.return_value.provision.assert_called_once_with(preserved_only=True)
        secret.assert_called_once_with(p, 'fixture-account', preserved_only=True)
        actions = [call[2] for call in p.calls if call[:2] == ('compute', 'instances')]
        self.assertEqual(actions, ['describe', 'stop', 'set-service-account', 'start'])
        self.assertFalse(any('create' in call for call in p.calls))

    def test_fake_gcloud_rebinds_preserved_assets_without_creating_them(self):
        class Provider:
            install_id = 'a' * 12
            name = 'momso-' + install_id
            project = 'fixture-project'
            marker = 'momso-install-' + install_id
            def __init__(self): self.calls = []; self.account = False
            def run(self, *args, **kwargs):
                self.calls.append(args)
                labels = {'momso-install-id': self.install_id}
                if args[:3] == ('iam', 'roles', 'list'): return []
                if args[:3] == ('iam', 'service-accounts', 'list'):
                    return [{'email': 'mv-' + self.install_id + '@' + self.project + '.iam.gserviceaccount.com'}] if self.account else []
                if args[:3] == ('iam', 'service-accounts', 'create'): self.account = True; return {}
                if args[:3] == ('iam', 'service-accounts', 'describe'): return {'description': self.marker}
                if args[:3] == ('storage', 'buckets', 'list'): return [{'name': 'momso-backup-' + self.install_id}]
                if args[:3] == ('storage', 'buckets', 'describe'): return {'labels': labels, 'public_access_prevention': 'enforced', 'uniform_bucket_level_access': True}
                if args[:3] == ('kms', 'keyrings', 'list'): return [{'name': 'projects/123/locations/asia-northeast3/keyRings/momso-backup-' + self.install_id}]
                if args[:3] == ('kms', 'keys', 'list'): return [{'name': 'projects/123/locations/asia-northeast3/keyRings/momso-backup-' + self.install_id + '/cryptoKeys/backup', 'labels': labels}]
                if args[:2] == ('secrets', 'list'): return [{'name': 'projects/123/secrets/momso-vault-' + self.install_id, 'labels': labels}]
                if args[:3] == ('secrets', 'versions', 'list'): return [{'name': 'projects/123/secrets/vault/versions/1', 'state': 'ENABLED'}]
                if args[:3] == ('compute', 'instances', 'describe'): return {'labels': labels, 'serviceAccounts': []}
                return {}
        p = Provider()
        secret, backup = restore.prepare_identity(p)
        self.assertEqual(secret, 'momso-vault-' + p.install_id)
        self.assertEqual(backup['bucket'], 'momso-backup-' + p.install_id)
        self.assertFalse(any(call[:3] in (('storage', 'buckets', 'create'), ('kms', 'keyrings', 'create'), ('kms', 'keys', 'create')) or call[:2] == ('secrets', 'create') or call[:3] == ('secrets', 'versions', 'add') for call in p.calls))

    def test_mobile_installation_uses_short_names_without_losing_full_owner(self):
        class Provider:
            install_id = 'a' * 12 + 'b' * 12
        self.assertEqual(names(Provider())['backup_bucket'], 'momso-backup-' + 'a' * 12)
        self.assertEqual(names(Provider())['account'], 'mv-' + 'a' * 12)
        self.assertEqual(Provider.install_id, 'a' * 12 + 'b' * 12)

    def test_new_vm_remains_stopped_until_restore_finishes(self):
        class Provider:
            install_id = 'a' * 12
            name = 'momso-' + install_id
            def __init__(self): self.calls = []
            def run(self, *args, **kwargs):
                self.calls.append(args[:3])
                return [] if args[:3] == ('compute', 'instances', 'list') else {'id': '12345', 'labels': {'momso-install-id': self.install_id}}
            def prepare_network(self): self.calls.append('network')
            def prepare_vm(self): self.calls.append('vm')
            def ssh(self, *_): self.calls.append('quiet'); return b''
            def install(self, *_args, **kwargs): self.calls.append(('install', kwargs.get('start_server')))
        provider = Provider(); state = {}
        with patch.object(restore, 'save_state'), patch.object(restore, 'prepare_identity', return_value=('secret', {})), patch.object(restore, 'restore_files', return_value={}), patch('momso_anywhere_vm_services.install_scripts') as install_scripts, patch.object(restore, 'run_restore', return_value={'restored': True, 'restoredBackupId': 'slot-0@1', 'restoredAt': 'now'}):
            result = restore.prepare_restored_vm(provider, None, 'digest', b'bundle', None, state)
            self.assertFalse(install_scripts.call_args.kwargs['start_services'])
        self.assertTrue(result['restored'])
        self.assertIn(('install', False), provider.calls)
        self.assertEqual(state['restore_pending']['vmId'], '12345')

    def test_existing_unrecorded_vm_is_not_replaced_or_restored(self):
        class Provider:
            install_id = 'a' * 12
            name = 'momso-' + install_id
            def run(self, *_args, **_kwargs): return [{'id': '12345', 'name': self.name, 'labels': {'momso-install-id': self.install_id}}]
        with self.assertRaisesRegex(InstallError, 'restore_vm_exists'):
            restore.prepare_restored_vm(Provider(), None, None, None, None, {})

    def test_restore_uses_signed_workers_and_waits_for_service(self):
        class Provider:
            install_id = 'a' * 12
            project = 'fixture-project'
            def ssh(self, wrapper, payload):
                code = base64.b64decode(re.search(r"b64decode\('([^']+)'\)", wrapper).group(1)).decode()
                ast.parse(code, feature_version=(3, 11))
                self.assertions(code, json.loads(payload))
                return b'{"restored":true,"restoredBackupId":"slot-0@1","restoredAt":"2026-11-20T00:00:00Z"}'
            def assertions(self, code, data):
                assert set(data['files']) == {*RESTORE_FILES, 'vault-provision.py'}
                assert 'User=momso' in code and 'LoadCredential=momso-vault-identity' in code
                assert "['systemctl','start','momso-restore.service']" in code
                assert '--create-initial' not in code
        provider = Provider()
        backup = {'bucket': 'momso-backup-' + provider.install_id, 'key': 'projects/fixture-project/locations/asia-northeast3/keyRings/momso-backup-' + provider.install_id + '/cryptoKeys/backup'}
        self.assertTrue(restore.run_restore(provider, bundle(), 'momso-vault-' + provider.install_id, backup)['restored'])

    def test_missing_signed_files_and_foreign_resources_fail_before_ssh(self):
        class Provider:
            install_id = 'a' * 12
            project = 'fixture-project'
            def ssh(self, *_): raise AssertionError('must not connect')
        p = Provider()
        with self.assertRaisesRegex(InstallError, 'restore_assets_unverified'):
            restore.run_restore(p, bundle(), 'foreign', {'bucket': 'foreign', 'key': 'foreign'})
        with self.assertRaises((ValueError, tarfile.TarError)):
            restore.run_restore(p, b'bad', 'momso-vault-' + p.install_id, {'bucket': 'momso-backup-' + p.install_id, 'key': 'projects/fixture-project/locations/asia-northeast3/keyRings/momso-backup-' + p.install_id + '/cryptoKeys/backup'})

    def test_restore_failure_is_not_reported_as_success(self):
        class Provider:
            install_id = 'a' * 12
            project = 'fixture-project'
            def ssh(self, *_): return b'{"ok":false,"code":"restore_failed","detail":"key_unavailable"}'
        p = Provider()
        backup = {'bucket': 'momso-backup-' + p.install_id, 'key': 'projects/fixture-project/locations/asia-northeast3/keyRings/momso-backup-' + p.install_id + '/cryptoKeys/backup'}
        with self.assertRaisesRegex(InstallError, 'restore_failed: key_unavailable'):
            restore.run_restore(p, bundle(), 'momso-vault-' + p.install_id, backup)


if __name__ == '__main__':
    unittest.main()
