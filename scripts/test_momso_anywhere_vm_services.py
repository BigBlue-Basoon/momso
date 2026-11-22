import ast
import base64
import json
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
                    raise services.InstallError('role not propagated')
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
