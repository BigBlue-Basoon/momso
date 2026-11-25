import unittest
from types import SimpleNamespace
from unittest.mock import patch

from momso_anywhere_pc_inventory import inspect_destroyed, confirmed_billing_zero
from momso_anywhere_vm_resources import names


class Tests(unittest.TestCase):
    def test_zero_requires_every_inventory_and_no_remaining_charge(self):
        names = ('vm', 'disk', 'network', 'subnet', 'firewall', 'router', 'nat', 'addresses',
                 'gateway', 'accounts', 'roles', 'secrets', 'iam', 'bucket', 'kms')
        groups = {name: {'complete': True, 'items': []} for name in names}
        resources = {'bucket': {'status': 'deleted'}, 'kms': {'status': 'deleted'}}
        self.assertTrue(confirmed_billing_zero(groups, [], resources, []))
        groups['addresses']['complete'] = False
        self.assertFalse(confirmed_billing_zero(groups, [], resources, []))
        groups['addresses'] = {'complete': True, 'items': ['fixture']}
        self.assertFalse(confirmed_billing_zero(groups, [], resources, []))
        groups['addresses']['items'] = []
        self.assertFalse(confirmed_billing_zero(groups, [], resources, [{'resource': 'kms'}]))
        resources['kms']['status'] = 'unverified'
        self.assertFalse(confirmed_billing_zero(groups, [], resources, []))
    def provider(self, run):
        return SimpleNamespace(install_id='a' * 12, marker='momso-install-' + 'a' * 12,
                               name='momso-' + 'a' * 12, network='fixture-net', subnet='fixture-sub',
                               router='fixture-router', firewall='fixture-firewall', run=run)

    def test_absent_resources_are_reported_without_writes(self):
        commands = []
        def run(*args, **_kwargs):
            commands.append(args)
            self.assertIn('list', args)
            return []
        result = inspect_destroyed(self.provider(run))
        self.assertTrue(commands)
        self.assertTrue(all(value['status'] == 'deleted' for name, value in result.items() if name != 'iam_bindings'))
        self.assertEqual(result['iam_bindings']['status'], 'unverified')

    def test_foreign_or_denied_results_stay_unverified(self):
        def run(*args, **_kwargs):
            if args[:2] == ('compute', 'instances'):
                return [{'name': 'momso-' + 'a' * 12, 'labels': {'momso-install-id': 'foreign'}}]
            if args[:2] == ('secrets', 'list'):
                raise OSError('denied')
            return []
        result = inspect_destroyed(self.provider(run))
        self.assertEqual(result['vm']['status'], 'unverified')
        self.assertEqual(result['secrets']['status'], 'unverified')

    def test_owned_remaining_resource_is_not_reported_deleted(self):
        def run(*args, **_kwargs):
            if args[:2] == ('compute', 'instances'):
                return [{'name': 'momso-' + 'a' * 12, 'labels': {'momso-install-id': 'a' * 12}}]
            return []
        self.assertEqual(inspect_destroyed(self.provider(run))['vm']['status'], 'retained')

    def test_backup_role_and_vault_are_counted_for_discard(self):
        def run(*args, **_kwargs):
            if args[:3] == ('iam', 'roles', 'list'):
                return [{'name': 'projects/test/roles/momsoBackup' + 'a' * 12, 'description': 'momso-install-' + 'a' * 12}]
            if args[:2] == ('secrets', 'list'):
                return [{'name': 'projects/test/secrets/momso-vault-' + 'a' * 12, 'labels': {'momso-install-id': 'a' * 12}}]
            return []
        result = inspect_destroyed(self.provider(run), discard_data=True)
        self.assertEqual(result['iam']['status'], 'retained')
        self.assertEqual(result['secrets']['status'], 'retained')
        self.assertEqual(inspect_destroyed(self.provider(run))['secrets']['status'], 'deleted')

    def test_new_declared_role_is_inventoried(self):
        p = self.provider(lambda *_args, **_kwargs: [])
        declared = names(p)
        declared['roles'] = (*declared['roles'], 'fixtureExtra' + p.install_id)
        def run(*args, **_kwargs):
            if args[:3] == ('iam', 'roles', 'list'):
                return [{'name': 'projects/test/roles/' + declared['roles'][-1], 'description': p.marker}]
            return []
        p.run = run
        with patch('momso_anywhere_pc_inventory.vm_resource_names', return_value=declared):
            self.assertEqual(inspect_destroyed(p, discard_data=True)['iam']['status'], 'retained')
            self.assertEqual(inspect_destroyed(p)['free_roles']['names'], [declared['roles'][-1]])

    def test_iam_inventory_reports_absent_present_and_failure(self):
        provider = self.provider(lambda *_args, **_kwargs: [])
        with patch('momso_anywhere_iam_pc.inspect_installation_bindings', return_value=[]):
            self.assertEqual(inspect_destroyed(provider)['iam_bindings']['status'], 'deleted')
        with patch('momso_anywhere_iam_pc.inspect_installation_bindings', return_value=['project']):
            self.assertEqual(inspect_destroyed(provider)['iam_bindings']['status'], 'retained')
        with patch('momso_anywhere_iam_pc.inspect_installation_bindings', side_effect=OSError('denied')):
            self.assertEqual(inspect_destroyed(provider)['iam_bindings'], {'status': 'unverified', 'reason': 'iam_inventory_failed'})

    def test_iam_read_detects_deleted_principal_and_custom_role(self):
        from momso_anywhere_iam_pc import inspect_installation_bindings
        provider = self.provider(lambda *_args, **_kwargs: {'etag': 'fixture', 'bindings': []})
        provider.project = 'fixture-project'
        resource = ((('projects', 'get-iam-policy', provider.project)), (), ())
        with patch('momso_anywhere_iam_pc.owned_policy_resources', return_value=[resource]), \
             patch('momso_anywhere_iam_pc.soft_deleted_buckets', return_value=[]):
            self.assertEqual(inspect_installation_bindings(provider), [])
            provider.run = lambda *_args, **_kwargs: {'etag': 'fixture', 'bindings': [{'role': 'roles/viewer',
                'members': ['deleted:serviceAccount:mv-' + provider.install_id + '@fixture-project.iam.gserviceaccount.com?uid=42']}]}
            self.assertEqual(len(inspect_installation_bindings(provider)), 1)
            provider.run = lambda *_args, **_kwargs: {'etag': 'fixture', 'bindings': [{'role':
                'projects/fixture-project/roles/momsoBackup' + provider.install_id, 'members': ['user:other@example.test']}]}
            self.assertEqual(len(inspect_installation_bindings(provider)), 1)
            provider.run = lambda *_args, **_kwargs: {'etag': 'fixture', 'bindings': [{'role':
                'projects/fixture-project/roles/momsoBackup' + provider.install_id + '_withcond_abcdef', 'members': ['user:other@example.test']}]}
            self.assertEqual(len(inspect_installation_bindings(provider)), 1)


if __name__ == '__main__':
    unittest.main()
