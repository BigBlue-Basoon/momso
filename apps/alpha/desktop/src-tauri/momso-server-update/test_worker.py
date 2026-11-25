import unittest
import tempfile
import sys
from pathlib import Path
from unittest.mock import patch

sys.path.insert(0, str(Path(__file__).resolve().parents[5] / "scripts"))
import worker


class WorkerTests(unittest.TestCase):
    def test_journal_limit_is_installed_and_conflicts_are_preserved(self):
        from unittest.mock import Mock
        with tempfile.TemporaryDirectory() as temporary:
            directory = Path(temporary) / "journal"
            run = Mock()
            self.assertEqual(worker.ensure_journal_limit(directory, run=run), "applied")
            path = directory / "90-momso.conf"
            self.assertEqual(path.read_bytes(), worker.JOURNAL_LIMIT)
            run.assert_called_once()
            path.write_text("custom setting")
            run.reset_mock()
            self.assertEqual(worker.ensure_journal_limit(directory, run=run), "conflict")
            self.assertEqual(path.read_text(), "custom setting")
            run.assert_not_called()

    def test_journal_symlink_is_not_followed(self):
        from unittest.mock import Mock
        with tempfile.TemporaryDirectory() as temporary:
            root = Path(temporary)
            target = root / "target"; target.mkdir()
            link = root / "journal"; link.symlink_to(target, target_is_directory=True)
            run = Mock()
            self.assertEqual(worker.ensure_journal_limit(link, run=run), "conflict")
            self.assertFalse((target / "90-momso.conf").exists())
            run.assert_not_called()
    def test_wires_channel_admission_gateway_and_progress(self):
        config = {"channel": "development", "updateRoot": "/tmp/update",
                  "statusFile": "/tmp/status", "serverTokenFile": "/tmp/token",
                  "lifecycleDir": "/tmp/lifecycle", "project": "fixture-project",
                  "region": "asia-northeast3", "gatewayService": "fixture-gateway",
                  "dataDir": "/tmp/data", "serverUnit": "server.service",
                  "proxyUnit": "proxy.service", "publicKey": "/tmp/key"}
        with patch.object(worker, "ServerAdmission") as admission, patch.object(worker, "CloudRunGateway") as gateway, \
             patch.object(worker, "SystemdHost") as host, patch.object(worker, "Updater") as updater, \
             patch.object(worker, "GitHubReleases") as source, patch.object(worker, "ReleasePoller") as poller:
            worker.make_poller(config)
            admission.assert_called_once_with("/tmp/token", "/tmp/lifecycle")
            gateway.assert_called_once_with("fixture-project", "asia-northeast3", "fixture-gateway")
            self.assertEqual(host.call_args.kwargs["proxy_unit"], "proxy.service")
            self.assertEqual(updater.call_args.kwargs["channel"], "development")
            source.assert_called_once_with("development")
            poller.assert_called_once()
