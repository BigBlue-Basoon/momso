import hashlib
from pathlib import Path
import subprocess
import tempfile
import unittest

import generate_momso_managed_history as generator
import momso_managed_history as history
from momso_server_install import GoogleProvider, InstallError
from unittest.mock import patch


class HistoryTests(unittest.TestCase):
    def test_history_contains_each_committed_blob_not_uncommitted_content(self):
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            def git(*args):
                return subprocess.run(['git', *args], cwd=root, check=True, capture_output=True)
            git('init', '-q')
            path = root / generator.FILES['momso_anywhere_backup.py']
            path.parent.mkdir(parents=True)
            def commit(value):
                path.write_bytes(value)
                git('add', '--', str(path.relative_to(root)))
                git('-c', 'user.name=Fixture', '-c', 'user.email=fixture@example.invalid', 'commit', '-qm', 'fixture')
            commit(b'first version\n')
            shared = root / generator.FILES['momso_anywhere_backup_google.py']
            shared.write_bytes(b'first version\n')
            git('add', '--', str(shared.relative_to(root)))
            git('-c', 'user.name=Fixture', '-c', 'user.email=fixture@example.invalid', 'commit', '-qm', 'shared fixture')
            main = git('branch', '--show-current').stdout.decode().strip()
            git('switch', '-qc', 'fixture-side')
            commit(b'side version\n')
            git('switch', '-q', main)
            commit(b'second version\n')
            git('-c', 'user.name=Fixture', '-c', 'user.email=fixture@example.invalid', 'merge', '-q', '-s', 'ours', 'fixture-side', '-m', 'fixture merge')
            path.write_bytes(b'uncommitted secret\n')
            generated = generator.generate(root)
            result = generated['files']['momso_anywhere_backup.py']
            self.assertEqual(result, sorted(hashlib.sha256(value).hexdigest() for value in (b'first version\n', b'side version\n', b'second version\n')))
            self.assertNotIn(hashlib.sha256(b'uncommitted secret\n').hexdigest(), result)
            self.assertIn(hashlib.sha256(b'first version\n').hexdigest(), generated['files']['momso_anywhere_backup_google.py'])

    def test_all_current_files_and_known_intermediate_versions_are_included(self):
        data = history.load()
        for group, mapping in (('files', generator.FILES), ('worker', generator.WORKER), ('proxy', generator.PROXY)):
            for name, relative in mapping.items():
                self.assertIn(hashlib.sha256((generator.ROOT / relative).read_bytes()).hexdigest(), data[group][name])
        for revision in ('a5c8ac582', 'd87fa55b0', 'b9dcba991', '5c2a87e3c'):
            for group, mapping in (('files', generator.FILES), ('worker', generator.WORKER), ('proxy', generator.PROXY)):
                for name, relative in mapping.items():
                    result = subprocess.run(['git', 'show', revision + ':' + relative], cwd=generator.ROOT, capture_output=True)
                    if result.returncode == 0:
                        self.assertIn(hashlib.sha256(result.stdout).hexdigest(), data[group][name], (revision, relative))

    def test_safe_conflict_reports_only_allowlisted_filename_and_reason(self):
        with patch('momso_server_install.shutil.which', return_value='gcloud'):
            provider = GoogleProvider('fixture@example.test', 'fixture-project', '0123456789ab', Path('fixture-key'))
        result = subprocess.CompletedProcess([], 1, stdout=b'', stderr=b'secret-value\nmomso-managed-conflict: momso_anywhere_backup.py: unrecognized\nmomso-managed-conflict: idle-token.py: not_regular\nmore-secret')
        with patch('momso_server_install.subprocess.run', return_value=result):
            with self.assertRaises(InstallError) as caught:
                provider.run('compute', 'ssh', 'fixture', '--command=x')
        self.assertEqual(caught.exception.managed_conflict, ('momso_anywhere_backup.py', 'unrecognized'))
        self.assertEqual(caught.exception.managed_conflicts, [('momso_anywhere_backup.py', 'unrecognized'), ('idle-token.py', 'not_regular')])
        self.assertEqual(len(str(caught.exception).splitlines()), 2)
        self.assertNotIn('secret-value', str(caught.exception))
        self.assertNotIn('more-secret', str(caught.exception))


if __name__ == '__main__':
    unittest.main()
