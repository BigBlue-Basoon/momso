from pathlib import Path
import os
import subprocess
import sys
import tempfile
import unittest
from unittest.mock import patch
from data_snapshots import DataSnapshots, LOCK_NAMES, reserve_large_write, estimate_snapshot_bytes


def reservation_in_another_process(root):
    program = '''
import sys
from data_snapshots import reserve_large_write
try:
    with reserve_large_write(sys.argv[1], 1):
        print('available')
except ValueError as error:
    print(str(error))
'''
    result = subprocess.run([sys.executable, '-c', program, str(root)], cwd=Path(__file__).parent,
                            capture_output=True, text=True, check=True, timeout=10)
    return result.stdout.strip()


class DataSnapshotsTests(unittest.TestCase):
    def test_restore_keeps_real_reservation_until_owner_exits(self):
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory); data = root / 'data'; data.mkdir()
            (data / 'database').write_text('before')
            snapshots = DataSnapshots(data, root / 'snapshots')
            saved = snapshots.snapshot()
            (data / 'database').write_text('after')
            with reserve_large_write(data, 123):
                inodes = [(data / name).stat().st_ino for name in LOCK_NAMES]
                self.assertEqual(reservation_in_another_process(data), 'reservation_busy')
                snapshots.restore(saved)
                self.assertEqual((data / 'database').read_text(), 'before')
                self.assertEqual([(data / name).stat().st_ino for name in LOCK_NAMES], inodes)
                self.assertEqual((data / LOCK_NAMES[1]).read_text(), '123\n')
                self.assertEqual(reservation_in_another_process(data), 'reservation_busy')
            self.assertEqual(reservation_in_another_process(data), 'available')

    def test_failed_restore_puts_original_locks_back_and_releases_them(self):
        for failure_stage in ('move_data', 'move_operation', 'install_replacement'):
            with self.subTest(stage=failure_stage), tempfile.TemporaryDirectory() as directory:
                root = Path(directory); data = root / 'data'; data.mkdir()
                (data / 'database').write_text('before')
                snapshots = DataSnapshots(data, root / 'snapshots')
                saved = snapshots.snapshot()
                (data / 'database').write_text('after')
                original_replace = os.replace
                def replace(source, destination):
                    source, destination = Path(source), Path(destination)
                    if (failure_stage == 'move_data' and source == data or
                        failure_stage == 'move_operation' and source.name == LOCK_NAMES[1] and source.parent.name == 'previous' or
                        failure_stage == 'install_replacement' and source.name == 'data' and destination == data):
                        raise OSError('injected restore failure')
                    return original_replace(source, destination)
                with reserve_large_write(data, 123):
                    inodes = [(data / name).stat().st_ino for name in LOCK_NAMES]
                    with patch('data_snapshots.os.replace', side_effect=replace):
                        with self.assertRaisesRegex(OSError, 'injected restore failure'):
                            snapshots.restore(saved)
                    self.assertEqual((data / 'database').read_text(), 'after')
                    self.assertEqual([(data / name).stat().st_ino for name in LOCK_NAMES], inodes)
                    self.assertEqual(reservation_in_another_process(data), 'reservation_busy')
                    self.assertEqual(list(root.glob('.momso-data-restore-*')), [])
                self.assertEqual(reservation_in_another_process(data), 'available')

    def test_reservation_excludes_second_writer_and_allows_after_exit(self):
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            (root / 'example').write_bytes(b'example')
            self.assertEqual(estimate_snapshot_bytes(root), 7)
            with reserve_large_write(root, 7):
                self.assertEqual((root / '.momso-reservation-operation.lock').read_text(), '7\n')
                with self.assertRaisesRegex(ValueError, 'reservation_busy'):
                    with reserve_large_write(root, 8):
                        pass
            with reserve_large_write(root, 8):
                pass

    def test_restore_during_held_reservation_leaves_usable_lock_files(self):
        # S167: a rollback replaces the data directory while the updater still
        # holds its reservation on the old files. The restored copies must be
        # valid lock files (regular, one link, owner-only) and another process
        # must be able to reserve once the updater lets go.
        import os, stat, subprocess, sys
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            data = root / 'data'; data.mkdir()
            (data / 'chat.db').write_bytes(b'x' * 16)
            with reserve_large_write(data, 1):
                pass
            snapshots = DataSnapshots(data, root / 'snapshots')
            snapshot = snapshots.snapshot()
            with reserve_large_write(data, 64):
                snapshots.restore(snapshot)
                for name in ('.momso-reservation-state.lock', '.momso-reservation-operation.lock'):
                    info = os.lstat(data / name)
                    self.assertTrue(stat.S_ISREG(info.st_mode)); self.assertEqual(info.st_nlink, 1)
                    self.assertEqual(info.st_mode & 0o077, 0)
            code = ("import sys; sys.path.insert(0, sys.argv[1]); from data_snapshots import reserve_large_write\n"
                    "with reserve_large_write(sys.argv[2], 1): print('reserved')")
            result = subprocess.run([sys.executable, '-c', code, str(Path(__file__).resolve().parent), str(data)],
                                    capture_output=True, text=True, timeout=30)
            self.assertEqual(result.stdout.strip(), 'reserved', result.stderr)

    def test_snapshot_restores_database_vault_and_archive_without_following_links(self):
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory); data = root / "data"; data.mkdir()
            for filename in ("database.sqlite", "vault.bin", "archive.md"):
                (data / filename).write_text("old-" + filename)
            outside = root / "outside"; outside.write_text("untouched")
            (data / "link").symlink_to(data / "archive.md")
            snapshots = DataSnapshots(data, root / "snapshots")
            saved = snapshots.snapshot()
            (data / "database.sqlite").write_text("migrated")
            (data / "vault.bin").write_text("changed")
            snapshots.restore(saved)
            self.assertEqual((data / "database.sqlite").read_text(), "old-database.sqlite")
            self.assertEqual((data / "vault.bin").read_text(), "old-vault.bin")
            self.assertEqual((data / "archive.md").read_text(), "old-archive.md")
            self.assertTrue((data / "link").is_symlink())
            self.assertEqual(outside.read_text(), "untouched")
            snapshots.restore(saved)

    def test_codex_temporary_external_links_do_not_block_snapshot(self):
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory); data = root / "data"; data.mkdir()
            (data / "database.sqlite").write_text("old")
            codex = data / "codex"; codex.mkdir()
            (codex / "auth.json").write_text("fixture")
            temporary = codex / "tmp" / "arg0-linux" / "codex-arg0fixture"
            temporary.mkdir(parents=True)
            (temporary / "codex-linux-sandbox").symlink_to(root / "outside-codex")
            snapshots = DataSnapshots(data, root / "snapshots")
            saved = snapshots.snapshot()
            self.assertEqual((Path(saved) / "data/codex/auth.json").read_text(), "fixture")
            self.assertFalse((Path(saved) / "data/codex/tmp").exists())
            (data / "database.sqlite").write_text("changed")
            snapshots.restore(saved)
            self.assertEqual((data / "database.sqlite").read_text(), "old")
            self.assertEqual((data / "codex/auth.json").read_text(), "fixture")

    def test_external_link_is_not_silently_omitted(self):
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory); data = root / "data"; data.mkdir()
            outside = root / "outside"; outside.write_text("untouched")
            (data / "link").symlink_to(outside)
            with self.assertRaisesRegex(ValueError, "external_data_symlink"):
                DataSnapshots(data, root / "snapshots").snapshot()
            self.assertEqual(outside.read_text(), "untouched")

    def test_rejects_outside_snapshot(self):
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory); (root / "data").mkdir()
            snapshots = DataSnapshots(root / "data", root / "snapshots")
            with self.assertRaisesRegex(ValueError, "invalid_snapshot"):
                snapshots.restore(root / "outside")


if __name__ == "__main__": unittest.main()
