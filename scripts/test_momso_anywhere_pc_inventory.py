import unittest
from types import SimpleNamespace

from momso_anywhere_pc_inventory import inspect_destroyed


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


if __name__ == '__main__':
    unittest.main()
