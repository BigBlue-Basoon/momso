import unittest
import io
import json
import tempfile
from contextlib import redirect_stdout
from pathlib import Path
from unittest.mock import patch

from momso_anywhere_deletion_preview import preview
from momso_server_install import InstallError
import momso_server_install


class Provider:
    install_id = 'a' * 12
    project = 'fixture-project'
    def __init__(self, foreign=False, offline=False):
        self.foreign, self.offline, self.calls = foreign, offline, []
    def run(self, *args, **kwargs):
        self.calls.append(args)
        if self.offline:
            raise InstallError('fixture unavailable')
        if args[:3] == ('storage', 'buckets', 'list'):
            return [{'name': 'momso-backup-' + self.install_id}]
        if args[:3] == ('storage', 'buckets', 'describe'):
            return {'name': 'momso-backup-' + self.install_id, 'labels': {'momso-install-id': 'b' * 12 if self.foreign else self.install_id}}
        if args[:3] == ('storage', 'objects', 'list'):
            return [{'name': 'momso-backup/slot-1.enc', 'updated': '2026-11-24T00:00:00Z'}]
        if args[:3] == ('kms', 'keys', 'list'):
            return [{'name': f'projects/{self.project}/locations/asia-northeast3/keyRings/momso-backup-{self.install_id}/cryptoKeys/backup', 'labels': {'momso-install-id': self.install_id}}]
        if args[:4] == ('kms', 'keys', 'versions', 'list'):
            return [{'state': 'ENABLED', 'protectionLevel': 'SOFTWARE'}]
        raise AssertionError(args)


class PreviewTests(unittest.TestCase):
    def test_backup_time_and_cost_without_mutations(self):
        provider = Provider()
        value = preview(provider)
        self.assertTrue(value['backupExists'])
        self.assertEqual(value['lastBackupAt'], '2026-11-24T00:00:00Z')
        self.assertEqual(value['estimatedMonthly']['kms'], 0.06)
        self.assertIsNone(value['estimatedMonthly']['total'])
        self.assertTrue(all('delete' not in call for call in provider.calls))

    def test_foreign_ownership_is_rejected_and_offline_is_unknown(self):
        with self.assertRaises(InstallError):
            preview(Provider(foreign=True))
        value = preview(Provider(offline=True))
        self.assertIsNone(value['backupExists'])
        self.assertIsNone(value['estimatedMonthly']['kms'])

    def test_pc_command_uses_existing_state_without_creating_keys(self):
        with tempfile.TemporaryDirectory() as directory:
            state = Path(directory) / 'state.json'
            state.write_text(json.dumps({'version': 1, 'id': 'a' * 12, 'account': 'fixture@example.test', 'project': 'fixture-project', 'steps': {}}), encoding='utf-8')
            provider = Provider()
            with patch.object(momso_server_install, 'GoogleProvider', return_value=provider), redirect_stdout(io.StringIO()) as output:
                code = momso_server_install.main(['delete-preview', '--account', 'fixture@example.test', '--project', 'fixture-project', '--state', str(state)])
            self.assertEqual(code, 0)
            self.assertTrue(json.loads(output.getvalue())['backupExists'])
            self.assertEqual([p.name for p in Path(directory).iterdir()], ['state.json'])

    def test_empty_current_and_soft_listing_does_not_disprove_old_versions(self):
        class Empty(Provider):
            def run(self, *args, **kwargs):
                if args[:3] == ('storage', 'objects', 'list'):
                    return []
                return super().run(*args, **kwargs)
        self.assertIsNone(preview(Empty())['backupExists'])


if __name__ == '__main__':
    unittest.main()
