import unittest
from types import SimpleNamespace
from unittest.mock import patch

from momso_anywhere_pc_inventory import inspect_destroyed
from momso_anywhere_vm_resources import names


class Tests(unittest.TestCase):
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


if __name__ == '__main__':
    unittest.main()
