import importlib.util
from pathlib import Path
import subprocess
import tarfile
import tempfile
import unittest
from types import SimpleNamespace
from unittest.mock import MagicMock, patch

spec = importlib.util.spec_from_file_location('build_public_s124', Path(__file__).with_name('build-public-desktop-s124.py'))
build = importlib.util.module_from_spec(spec)
spec.loader.exec_module(build)


class PublicBuildTests(unittest.TestCase):
    def test_timeout_releases_lock_and_retries_same_source(self):
        first = MagicMock(); first.pid = 101
        first.poll.side_effect = [None, None]
        second = MagicMock(); second.pid = 102; second.poll.return_value = 0; second.returncode = 0
        lock = SimpleNamespace(returncode=0, stdout='fixture-owner\n')
        with patch.object(build, 'available_ram', return_value=8 * 1024**3), \
             patch.object(build.shutil, 'disk_usage', return_value=SimpleNamespace(free=80 * 1024**3)), \
             patch.object(build.subprocess, 'run', return_value=lock) as command, \
             patch.object(build.subprocess, 'check_output', return_value='fixture-owner\n'), \
             patch.object(build.subprocess, 'Popen', side_effect=[first, second]) as launch, \
             patch.object(build.time, 'monotonic', side_effect=[0, 4440, 5000]), \
             patch.object(build, 'run') as release_lock:
            build.build_with_lock(Path('same-source'), {})
        self.assertEqual(launch.call_count, 2)
        self.assertEqual(launch.call_args_list[0].kwargs['cwd'], launch.call_args_list[1].kwargs['cwd'])
        self.assertEqual(release_lock.call_count, 2)
        self.assertTrue(any('taskkill' in str(call) for call in command.call_args_list))

    def test_git_archive_must_match_commit_bytes(self):
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory); repo = root / 'repo'; repo.mkdir()
            def git(*args):
                return subprocess.run(['git', '-C', str(repo), *args], check=True, capture_output=True)
            git('init', '-q')
            git('config', 'user.email', 'fixture@example.invalid')
            git('config', 'user.name', 'Fixture')
            (repo / 'source.txt').write_bytes(b'first\nsecond\n')
            git('add', 'source.txt'); git('commit', '-qm', 'fixture')
            archive = root / 'source.tar'
            git('-c', 'core.autocrlf=false', 'archive', '--format=tar', '-o', str(archive), 'HEAD')
            source = root / 'source'; source.mkdir()
            with tarfile.open(archive) as tar:
                tar.extractall(source, filter='data')
            build.require_commit_tree(repo, 'HEAD', source)
            (source / 'source.txt').write_bytes(b'first\r\nsecond\r\n')
            with self.assertRaisesRegex(ValueError, 'bytes differ'):
                build.require_commit_tree(repo, 'HEAD', source)

    def test_original_sources_must_not_change_during_build(self):
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            source = root / 'source.txt'
            source.write_text('original', encoding='utf8')
            files = build.tracked_files(root)
            (root / 'generated.txt').write_text('generated', encoding='utf8')
            build.require_unchanged(root, files)
            source.write_text('changed', encoding='utf8')
            with self.assertRaisesRegex(ValueError, 'changed'):
                build.require_unchanged(root, files)

    def test_generated_file_cannot_be_added_to_application_source(self):
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            source = root / 'apps/alpha/desktop/src'
            source.mkdir(parents=True)
            (source / 'app.ts').write_text('original', encoding='utf8')
            files = build.tracked_files(root)
            (source / 'extra.ts').write_text('untracked', encoding='utf8')
            with self.assertRaisesRegex(ValueError, 'uncommitted'):
                build.require_unchanged(root, files)


if __name__ == '__main__':
    unittest.main()
