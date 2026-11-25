import json
from pathlib import Path
import tempfile
import unittest
from unittest.mock import patch

from layout_migration import prepare_baseline, prepare_verified_baseline
from test_update import UpdateTests
from update import UpdateError, _extract
from momso_anywhere_proxy_files import PROXY_FILES


class LayoutMigrationTests(unittest.TestCase):
    def setUp(self):
        self.tmp = tempfile.TemporaryDirectory()
        self.flat = Path(self.tmp.name) / "flat"
        self.flat.mkdir()
        for name in ("momso-server", *PROXY_FILES):
            (self.flat / name).write_text(name)
        (self.flat / "web").mkdir()
        (self.flat / "web" / ".momso-owned").write_text("owned")
        (self.flat / "web" / "index.html").write_text("old")
        self.root = self.flat / "releases"

    def tearDown(self):
        self.tmp.cleanup()

    def test_prepares_baseline_without_changing_running_files(self):
        baseline = prepare_baseline(self.flat, self.root, 7)
        self.assertEqual((self.flat / "momso-server").read_text(), "momso-server")
        self.assertFalse((self.flat / "momso-server").is_symlink())
        self.assertEqual(baseline.stat().st_mode & 0o777, 0o755)
        self.assertEqual(self.root.stat().st_mode & 0o777, 0o755)
        self.assertEqual((self.root / "current").resolve(), baseline)
        for name in PROXY_FILES:
            self.assertEqual((baseline / name).read_bytes(), (self.flat / name).read_bytes())
        self.assertEqual(json.loads((self.root / "installed.json").read_text())["sequence"], 7)
        self.assertEqual(prepare_baseline(self.flat, self.root, 7), baseline)

    def test_resumes_interruption_after_sequence_is_written(self):
        with patch("layout_migration.os.replace", side_effect=KeyboardInterrupt):
            # Avoid interrupting the atomic sequence write itself.
            with patch("layout_migration._atomic_json") as write:
                def write_sequence(path, value):
                    path.write_text(json.dumps(value))
                write.side_effect = write_sequence
                with self.assertRaises(KeyboardInterrupt):
                    prepare_baseline(self.flat, self.root, 7)
        self.assertEqual((prepare_baseline(self.flat, self.root, 7) / "web" / "index.html").read_text(), "old")

    def test_rejects_unknown_sequence_and_links(self):
        with self.assertRaisesRegex(ValueError, "installed_sequence_required"):
            prepare_baseline(self.flat, self.root, 0)
        (self.flat / "web" / "link").symlink_to(self.flat / "momso-server")
        with self.assertRaisesRegex(ValueError, "invalid_flat_installation"):
            prepare_baseline(self.flat, self.root, 7)


class VerifiedBaselineTests(UpdateTests):
    def test_installed_sequence_comes_from_matching_signed_bundle(self):
        flat = self.root / "installed-flat"
        flat.mkdir()
        _extract((self.server_dir / "bundle.tar.gz").read_bytes(), flat)
        (flat / "web" / ".momso-owned").write_text("owned")
        baseline = prepare_verified_baseline(flat, self.updater, self.base + "/manifest.json", self.base + "/manifest.sig")
        self.assertEqual((baseline / "momso-server").read_text(), "momso-server")
        self.assertEqual(json.loads((self.updater.root / "installed.json").read_text())["sequence"], 2)
        self.assertEqual(prepare_verified_baseline(flat, self.updater, self.base + "/manifest.json", self.base + "/manifest.sig"), baseline)

    def test_mismatched_installed_files_never_initialize_sequence(self):
        flat = self.root / "installed-flat"
        flat.mkdir()
        _extract((self.server_dir / "bundle.tar.gz").read_bytes(), flat)
        (flat / "web" / ".momso-owned").write_text("owned")
        (flat / "momso-server").write_text("other")
        with self.assertRaisesRegex(UpdateError, "installed_release_mismatch"):
            prepare_verified_baseline(flat, self.updater, self.base + "/manifest.json", self.base + "/manifest.sig")
        self.assertFalse((self.updater.root / "installed.json").exists())
