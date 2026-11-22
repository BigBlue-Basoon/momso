import ast
import base64
import hashlib
import io
import json
import os
from pathlib import Path
import subprocess
import tempfile
import unittest
from unittest.mock import patch

import momso_anywhere_vm_services as services


class FakeProvider:
    project = 'fixture-project'
    install_id = '0123456789ab'
    name = 'fixture-vm'
    marker = 'fixture-owner'

    def __init__(self):
        self.calls = []
        self.vm = {'labels': {'momso-install-id': self.install_id}, 'serviceAccounts': []}
        self.accounts = []
        self.roles = []
        self.secrets = []
        self.versions = []
        self.payloads = []

    def run(self, *args, **kwargs):
        self.calls.append(args)
        if args[:3] == ('compute', 'instances', 'describe'):
            return self.vm
        if args[:3] == ('iam', 'service-accounts', 'list'):
            return self.accounts
        if args[:3] == ('iam', 'service-accounts', 'create'):
            self.accounts = [{'email': 'mv-' + self.install_id + '@' + self.project + '.iam.gserviceaccount.com'}]
        if args[:3] == ('iam', 'service-accounts', 'describe'):
            return {'description': self.marker}
        if args[:3] == ('iam', 'roles', 'list'):
            return self.roles
        if args[:3] == ('iam', 'roles', 'create'):
            self.roles = [{'name': 'projects/x/roles/momsoStop' + self.install_id}]
        if args[:3] == ('iam', 'roles', 'describe'):
            return {'description': self.marker, 'includedPermissions': ['compute.instances.stop']}
        if args[:2] == ('secrets', 'list'):
            return self.secrets
        if args[:2] == ('secrets', 'create'):
            self.secrets = [{'name': 'projects/x/secrets/' + args[2], 'labels': {'momso-install-id': self.install_id}}]
        if args[:3] == ('secrets', 'versions', 'list'):
            return self.versions
        if args[:3] == ('compute', 'instances', 'set-service-account'):
            self.vm['serviceAccounts'] = [{'email': 'mv-' + self.install_id + '@' + self.project + '.iam.gserviceaccount.com', 'scopes': [services.SCOPE]}]
        return {}

    def ssh(self, wrapper, data=None):
        self.calls.append(('ssh',))
        node = ast.parse(wrapper)
        encoded = next(n.args[0].value for n in ast.walk(node) if isinstance(n, ast.Call) and isinstance(n.func, ast.Attribute) and n.func.attr == 'b64decode')
        payload = base64.b64decode(encoded).decode()
        ast.parse(payload)
        if data is not None:
            self.payloads.append(payload)
            self.versions = [{'name': '1'}]
            self.files = json.loads(data)

    def health(self):
        self.calls.append(('health',))


class InstallerTests(unittest.TestCase):
    @unittest.skipIf(os.name == 'nt', 'VM payload requires Linux filesystem operations')
    def test_first_migration_adopts_generated_files_preserves_unmanaged_backups(self):
        with tempfile.TemporaryDirectory() as temp:
            base = Path(temp)
            root = base / 'opt'; root.mkdir(mode=0o700)
            private = base / 'private'; private.mkdir(mode=0o700)
            systemd = base / 'systemd'; systemd.mkdir(mode=0o700)
            overrides = systemd / 'momso-server.service.d'; overrides.mkdir(mode=0o700)
            checkpoint = root / '.momso-upgrade-backup'; checkpoint.mkdir(mode=0o700)
            (checkpoint / 'owner').write_text('momso-upgrade-v1')
            generated = [(root, 'idle-token.py'), (systemd, 'momso-vault-credential.service'), (systemd, 'momso-idle-stop.service'), (overrides, 'vault.conf'), (overrides, 'backup.conf')]
            for directory, name in generated:
                (directory / name).write_bytes(b'legacy generated content')
            preserved = ('momso-server.06368bcfc-failed', 'momso-server.s32-ok', 'web.06368bcfc-failed', 'web.s32-ok')
            for name in preserved:
                path = root / name
                if name.startswith('web.'):
                    path.mkdir(); path = path / 'index.html'
                path.write_bytes(b'user recovery data')
            p = FakeProvider()
            with patch.object(services, '_paths', return_value={'momso_anywhere_backup.py': b'new worker'}):
                services.install_scripts(p, 'fixture-secret', False, {'bucket': 'fixture', 'key': 'fixture'})
            code = p.payloads[-1].replace('/opt/momso', str(root)).replace('/etc/momso-private', str(private)).replace('/etc/systemd/system', str(systemd))
            stdin = io.TextIOWrapper(io.BytesIO(json.dumps(p.files).encode()))
            with patch('sys.stdin', stdin), patch('shutil.which', return_value='/usr/bin/fixture'), patch.object(subprocess, 'run', return_value=subprocess.CompletedProcess([], 0)):
                exec(compile(code, '<vm fixture>', 'exec'), {})
            for directory, name in generated:
                self.assertNotEqual((directory / name).read_bytes(), b'legacy generated content')
            for name in preserved:
                path = root / name
                self.assertEqual((path / 'index.html' if path.is_dir() else path).read_bytes(), b'user recovery data')
            for tag in ('helper', 'units', 'overrides'):
                self.assertTrue((private / ('managed-' + tag + '.json')).is_file())
                self.assertTrue((checkpoint / ('managed-' + tag + '-journal.json')).is_file())

    @unittest.skipIf(os.name == 'nt', 'VM payload requires Linux filesystem operations')
    def test_preflight_reports_all_groups_without_changing_files(self):
        with tempfile.TemporaryDirectory() as temp:
            base = Path(temp)
            root = base / 'opt'; root.mkdir(mode=0o700)
            private = base / 'private'; private.mkdir(mode=0o700)
            systemd = base / 'systemd'; systemd.mkdir(mode=0o700)
            overrides = systemd / 'momso-server.service.d'; overrides.mkdir(mode=0o700)
            (root / 'momso_anywhere_backup.py').write_bytes(b'unknown worker')
            (root / 'idle-token.py').write_bytes(b'unknown helper')
            (systemd / 'momso-idle-stop.service').write_bytes(b'unknown unit')
            p = FakeProvider()
            with patch.object(services, '_paths', return_value={'momso_anywhere_backup.py': b'new'}):
                services.install_scripts(p, 'fixture-secret', False, {'bucket': 'fixture', 'key': 'fixture'})
            # An existing main ledger disables one-time adoption for all groups.
            (private / 'managed-files.json').write_text(json.dumps({'version': 1, 'owner': p.install_id, 'files': {}}))
            (private / 'managed-units.json').write_text('invalid fixture')
            code = p.payloads[-1].replace('/opt/momso', str(root)).replace('/etc/momso-private', str(private)).replace('/etc/systemd/system', str(systemd))
            stdin = io.TextIOWrapper(io.BytesIO(json.dumps(p.files).encode()))
            with patch('sys.stdin', stdin), patch.object(subprocess, 'run') as run:
                with self.assertRaises(RuntimeError) as caught:
                    exec(compile(code, '<vm fixture>', 'exec'), {})
            for name in ('momso_anywhere_backup.py', 'idle-token.py', 'momso-idle-stop.service'):
                self.assertIn('momso-managed-conflict: ' + name + ': unrecognized', str(caught.exception))
            self.assertIn('momso-managed-conflict: managed-units.json: manifest_invalid', str(caught.exception))
            self.assertEqual((root / 'momso_anywhere_backup.py').read_bytes(), b'unknown worker')
            self.assertFalse((private / 'managed-helper.json').exists())
            run.assert_not_called()

    def test_unrecognized_file_is_reported_without_retrying(self):
        p = FakeProvider()
        conflict = services.InstallError('fixture', managed_conflict=('momso_anywhere_backup.py', 'unrecognized'))
        with patch.object(p, 'ssh', side_effect=conflict) as ssh, patch.object(services.time, 'sleep') as sleep:
            with self.assertRaises(services.InstallError) as caught:
                services.install_scripts(p, 'fixture-secret', False, {'bucket': 'fixture', 'key': 'fixture'})
        self.assertEqual(caught.exception.managed_conflict, ('momso_anywhere_backup.py', 'unrecognized'))
        ssh.assert_called_once()
        sleep.assert_not_called()

    @unittest.skipIf(os.name == 'nt', 'VM payload requires Linux filesystem operations')
    def test_legacy_worker_upgrade_records_the_replacement(self):
        with tempfile.TemporaryDirectory() as temp:
            base = Path(temp)
            root = base / 'opt'; root.mkdir()
            private = base / 'private'; private.mkdir()
            systemd = base / 'systemd'; systemd.mkdir()
            checkpoint = root / '.momso-upgrade-backup'; checkpoint.mkdir()
            for folder in (root, private, systemd, checkpoint): folder.chmod(0o700)
            (checkpoint / 'owner').write_text('momso-upgrade-v1')
            old = b'old worker'; new = b'new worker'
            (root / 'momso_anywhere_backup.py').write_bytes(old)
            p = FakeProvider()
            with patch.object(services, '_paths', return_value={'momso_anywhere_backup.py': new}), patch.object(services, 'LEGACY_FILES', {'momso_anywhere_backup.py': [hashlib.sha256(old).hexdigest()]}):
                services.install_scripts(p, 'fixture-secret', False, {'bucket': 'fixture', 'key': 'fixture'})
            code = p.payloads[-1].replace('/opt/momso', str(root)).replace('/etc/momso-private', str(private)).replace('/etc/systemd/system', str(systemd))
            stdin = io.TextIOWrapper(io.BytesIO(json.dumps(p.files).encode()))
            with patch('sys.stdin', stdin), patch('shutil.which', return_value='/usr/bin/fixture'), patch.object(subprocess, 'run', return_value=subprocess.CompletedProcess([], 0)):
                exec(compile(code, '<vm fixture>', 'exec'), {})
            self.assertEqual((root / 'momso_anywhere_backup.py').read_bytes(), new)
            recorded = json.loads((private / 'managed-files.json').read_text())
            self.assertEqual(recorded['files']['momso_anywhere_backup.py'], hashlib.sha256(new).hexdigest())
            self.assertTrue((checkpoint / 'managed-files.json').is_file())

    @unittest.skipIf(os.name == 'nt', 'VM payload requires Linux filesystem operations')
    def test_legacy_worker_upgrade_rolls_back_when_service_restart_fails(self):
        with tempfile.TemporaryDirectory() as temp:
            base = Path(temp)
            root = base / 'opt'; root.mkdir()
            private = base / 'private'; private.mkdir()
            systemd = base / 'systemd'; systemd.mkdir()
            checkpoint = root / '.momso-upgrade-backup'; checkpoint.mkdir()
            for folder in (root, private, systemd, checkpoint): folder.chmod(0o700)
            (checkpoint / 'owner').write_text('momso-upgrade-v1')
            old = b'old worker'; new = b'new worker'
            (root / 'momso_anywhere_backup.py').write_bytes(old)
            p = FakeProvider()
            with patch.object(services, '_paths', return_value={'momso_anywhere_backup.py': new}), patch.object(services, 'LEGACY_FILES', {'momso_anywhere_backup.py': [hashlib.sha256(old).hexdigest()]}):
                services.install_scripts(p, 'fixture-secret', False, {'bucket': 'fixture', 'key': 'fixture'})
            code = p.payloads[-1].replace('/opt/momso', str(root)).replace('/etc/momso-private', str(private)).replace('/etc/systemd/system', str(systemd))
            def run(args, **kwargs):
                if tuple(args[:2]) == ('systemctl', 'daemon-reload'):
                    raise subprocess.CalledProcessError(1, args)
                return subprocess.CompletedProcess(args, 0)
            stdin = io.TextIOWrapper(io.BytesIO(json.dumps(p.files).encode()))
            with patch('sys.stdin', stdin), patch('shutil.which', return_value='/usr/bin/fixture'), patch.object(subprocess, 'run', side_effect=run):
                with self.assertRaises(subprocess.CalledProcessError):
                    exec(compile(code, '<vm fixture>', 'exec'), {})
            self.assertEqual((root / 'momso_anywhere_backup.py').read_bytes(), old)
            self.assertFalse((private / 'managed-files.json').exists())
            self.assertFalse((checkpoint / 'managed-files.json').exists())

    def test_new_and_upgrade_preserve_key_and_install_services(self):
        p = FakeProvider()
        with patch.object(services.time, 'sleep'):
            services.install(p)
            services.install(p)
        self.assertEqual(sum(c[:3] == ('compute', 'instances', 'stop') for c in p.calls), 1)
        self.assertIn('if True:cmd.append', p.payloads[0])
        self.assertIn('if False:cmd.append', p.payloads[1])
        self.assertIn('LoadCredential=momso-vault-identity', p.payloads[1])
        self.assertIn('momso-idle-stop.service', p.payloads[1])
        self.assertEqual(set(p.files), {'vault-provision.py', 'idle-stop.py',
                         'momso_anywhere_backup.py', 'momso_anywhere_backup_agent.py',
                         'momso_anywhere_backup_google.py', 'momso_anywhere_vault_identity.py'})
        self.assertIn('MOMSO_BACKUP_AGENT=/opt/momso/momso_anywhere_backup_agent.py', p.payloads[1])
        self.assertIn('MOMSO_BACKUP_KMS_KEY=', p.payloads[1])
        self.assertTrue(any(c[:3] == ('storage', 'buckets', 'create') for c in p.calls))
        self.assertEqual(sum(c[:2] == ('secrets', 'remove-iam-policy-binding') for c in p.calls), 2)

    def test_unowned_vm_rejected_before_cloud_writes(self):
        p = FakeProvider()
        p.vm['labels'] = {}
        with self.assertRaises(services.InstallError): services.install(p)
        self.assertEqual(len(p.calls), 1)

    def test_foreign_account_is_not_replaced(self):
        p = FakeProvider()
        p.vm['serviceAccounts'] = [{'email': 'foreign@example.test', 'scopes': []}]
        with self.assertRaises(services.InstallError): services.install(p)
        self.assertFalse(any(c[:3] == ('compute', 'instances', 'stop') for c in p.calls))
        self.assertFalse(any(c[:2] == ('secrets', 'remove-iam-policy-binding') for c in p.calls))

    def test_restart_failure_still_releases_owned_gate(self):
        p = FakeProvider()
        with patch.object(services, 'install_scripts', side_effect=services.InstallError('fixture')), patch.object(services, '_release_gate') as release:
            with self.assertRaises(services.InstallError):
                services.install(p)
        release.assert_called_once_with(p)

    def test_new_role_binding_retries_propagation_delay(self):
        p = FakeProvider()
        run = p.run
        attempts = []
        def delayed(*args, **kwargs):
            if args[:3] == ('compute', 'instances', 'add-iam-policy-binding'):
                attempts.append(1)
                if len(attempts) < 4:
                    raise services.InstallError('role not propagated', role_propagating=True)
            return run(*args, **kwargs)
        with patch.object(p, 'run', side_effect=delayed), patch.object(services.time, 'sleep') as sleep:
            services.identity(p)
        self.assertEqual(len(attempts), 4)
        self.assertEqual(sleep.call_count, 3)

    def test_role_has_only_instance_stop(self):
        p = FakeProvider()
        services.identity(p)
        role = next(c for c in p.calls if c[:3] == ('iam', 'roles', 'create'))
        self.assertIn('--permissions=compute.instances.stop', role)


if __name__ == '__main__': unittest.main()
