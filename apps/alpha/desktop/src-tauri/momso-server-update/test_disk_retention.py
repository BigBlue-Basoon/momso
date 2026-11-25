import tempfile
import time
import unittest
import sys
from pathlib import Path
from types import SimpleNamespace
from unittest.mock import Mock, patch

sys.path.insert(0, str(Path(__file__).resolve().parents[5] / "scripts"))
from systemd_host import SystemdHost
from update import Updater, UpdateError, _atomic_json


class RetentionTests(unittest.TestCase):
    def test_only_owned_old_releases_and_unneeded_snapshots_are_removed(self):
        with tempfile.TemporaryDirectory() as temporary:
            root = Path(temporary)
            host = SystemdHost.__new__(SystemdHost)
            host.root = root
            host.current = root / "current"
            host.snapshots = SimpleNamespace(snapshots=root / "data-snapshots")
            host.snapshots.snapshots.mkdir()
            def release(name, owned=True):
                path = root / name
                (path / "web").mkdir(parents=True)
                if owned: (path / "web" / ".momso-owned").write_text("momso")
                return path
            old = release("release-2-aaaaaaaa")
            new = release("release-3-bbbbbbbb")
            obsolete = release("release-1-cccccccc")
            unmanaged = release("release-1-dddddddd", False)
            host.current.symlink_to(new, target_is_directory=True)
            snapshots = []
            for suffix in ("aaaaaaaa", "bbbbbbbb", "cccccccc", "dddddddd"):
                item = host.snapshots.snapshots / ("data-" + suffix)
                (item / "data").mkdir(parents=True)
                snapshots.append(item)
                time.sleep(0.01)
            record = {"phase": "complete", "needsRelease": False, "oldRelease": str(old),
                      "newRelease": str(new), "snapshot": str(snapshots[0])}
            with patch.dict("os.environ", {"MOMSO_UPDATE_SNAPSHOT_KEEP": "bad"}):
                with self.assertRaisesRegex(ValueError, "invalid_snapshot_retention"):
                    host.cleanup_old_updates(record)
            self.assertTrue(obsolete.exists())
            with patch.dict("os.environ", {"MOMSO_UPDATE_SNAPSHOT_KEEP": "2"}):
                host.cleanup_old_updates(record)
            self.assertTrue(old.exists()); self.assertTrue(new.exists())
            self.assertFalse(obsolete.exists()); self.assertTrue(unmanaged.exists())
            self.assertTrue(snapshots[0].exists()); self.assertFalse(snapshots[1].exists())
            self.assertTrue(snapshots[2].exists()); self.assertTrue(snapshots[3].exists())
            self.assertTrue((root / "cleanup-status.json").is_file())

    def test_unfinished_update_preserves_everything(self):
        host = SystemdHost.__new__(SystemdHost)
        host.cleanup_old_updates({"phase": "switching", "needsRelease": True})

    def test_low_disk_refuses_before_bundle_download(self):
        with tempfile.TemporaryDirectory() as temporary:
            host = Mock()
            host.current_sequence.return_value = 1
            updater = Updater(temporary, Path(temporary) / "key", host)
            updater.check = Mock(return_value={"sequence": 2, "gatewayImage": "fake"})
            with patch("update.shutil.disk_usage", return_value=SimpleNamespace(free=1)), \
                 patch("update._download") as download:
                with self.assertRaisesRegex(UpdateError, "disk_low"):
                    updater._install("manifest", "signature")
                download.assert_not_called()

    def test_low_disk_rechecks_after_safe_cleanup(self):
        with tempfile.TemporaryDirectory() as temporary:
            host = Mock()
            host.current_sequence.return_value = 1
            updater = Updater(temporary, Path(temporary) / "key", host)
            updater.check = Mock(return_value={"sequence": 2, "gatewayImage": "fake", "bundleUrl": "https://fixture"})
            _atomic_json(updater.state, {"phase": "complete", "needsRelease": False})
            with patch("update.shutil.disk_usage", side_effect=[SimpleNamespace(free=1), SimpleNamespace(free=2 * 1024**3)]), \
                 patch("update._download", side_effect=RuntimeError("after_cleanup")) as download:
                with self.assertRaisesRegex(RuntimeError, "after_cleanup"):
                    updater._install("manifest", "signature")
                host.cleanup_old_updates.assert_called_once()
                download.assert_called_once()


if __name__ == "__main__": unittest.main()
