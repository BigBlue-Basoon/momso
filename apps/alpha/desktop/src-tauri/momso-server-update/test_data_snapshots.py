from pathlib import Path
import tempfile
import unittest
from data_snapshots import DataSnapshots


class DataSnapshotsTests(unittest.TestCase):
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
