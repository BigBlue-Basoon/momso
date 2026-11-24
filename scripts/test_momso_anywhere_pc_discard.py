import json
import unittest
from types import SimpleNamespace
from unittest.mock import patch
from momso_anywhere_pc_discard import discard_backup
from momso_server_install import InstallError


class Tests(unittest.TestCase):
    def test_token_only_on_stdin(self):
        provider = SimpleNamespace(project='fixture-project', install_id='a' * 12, run=lambda *args: b'fixture-token')
        value = {'mode': 'delete_all', 'resources': {}, 'billable': [], 'billingZero': None}
        with patch('momso_anywhere_pc_discard.shutil.which', return_value='node'), patch('momso_anywhere_pc_discard.subprocess.run', return_value=SimpleNamespace(returncode=0, stdout=json.dumps(value).encode())) as run:
            self.assertEqual(discard_backup(provider), value)
            args, kwargs = run.call_args
            self.assertNotIn('fixture-token', str(args))
            self.assertEqual(json.loads(kwargs['input'])['token'], 'fixture-token')
            self.assertEqual(kwargs['stderr'], -3)

    def test_failed_auth_remains_unverified(self):
        def fail(*_args): raise InstallError('fixture')
        provider = SimpleNamespace(project='fixture-project', install_id='a' * 12, run=fail)
        with patch('momso_anywhere_pc_discard.shutil.which', return_value='node'):
            value = discard_backup(provider, {'billable': [{'resource': 'bucket', 'endsAt': '2026-12-01T00:00:00Z'}]})
        self.assertIsNone(value['billingZero'])
        self.assertEqual(value['resources']['bucket']['status'], 'unverified')
        self.assertEqual(value['billable'][0]['endsAt'], '2026-12-01T00:00:00Z')


if __name__ == '__main__': unittest.main()
