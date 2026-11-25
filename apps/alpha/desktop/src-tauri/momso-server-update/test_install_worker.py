import json
import os
from pathlib import Path
import subprocess
import tempfile
import unittest
from unittest.mock import Mock, patch

import install_worker


class InstallerTests(unittest.TestCase):
    def setUp(self):
        self.tmp = tempfile.TemporaryDirectory()
        root = Path(self.tmp.name)
        self.flat, self.units, self.worker = root / "flat", root / "units", root / "worker"
        self.flat.mkdir(); self.units.mkdir(); self.worker.mkdir()
        self.proxy = self.units / "momso-private-proxy.service"
        self.original = ("[Unit]\nRequires=momso-server.service\n[Service]\nExecStart=/usr/bin/python3 "
                         + str(self.flat) + "/momso_anywhere_vm_proxy.py\n").encode()
        self.proxy.write_bytes(self.original)
        self.config_path = root / "config.json"
        self.config = {"updateRoot": str(root / "releases"), "channel": "development"}
        self.updater = Mock()
        self.updater.host.acquire_maintenance.return_value = True

    def tearDown(self):
        self.tmp.cleanup()

    def invoke(self, run):
        with patch.object(install_worker, "prepare_verified_baseline") as baseline:
            install_worker.install(self.flat, self.units, self.worker, self.config_path,
                                   self.config, self.updater, "manifest", "sig", run=run)
            baseline.assert_called_once()

    def test_preserves_server_and_enables_separate_worker(self):
        run = Mock()
        self.invoke(run)
        self.assertNotIn(b"Requires=momso-server.service", self.proxy.read_bytes())
        self.assertIn(b"/current/momso_anywhere_vm_proxy.py", self.proxy.read_bytes())
        self.assertEqual(json.loads(self.config_path.read_text())["channel"], "development")
        self.assertEqual((Path(self.config["updateRoot"]) / ".momso-update-active").read_text(), "momso-update-v1\n")
        self.assertEqual(run.call_args_list[-1].args[0], ["systemctl", "enable", "--now", "momso-update.service"])
        self.updater.host.release_maintenance.assert_called_once()

    def test_failure_restores_original_units(self):
        run = Mock(side_effect=[None, subprocess.CalledProcessError(1, "systemctl"), None, None, None])
        with self.assertRaises(subprocess.CalledProcessError):
            self.invoke(run)
        self.assertEqual(self.proxy.read_bytes(), self.original)
        self.assertFalse(self.config_path.exists())
        self.assertFalse((self.units / "momso-update.service").exists())
        self.assertFalse((Path(self.config["updateRoot"]) / ".momso-update-active").exists())
        self.updater.host.release_maintenance.assert_called_once()

    def test_busy_does_not_change_any_files(self):
        self.updater.host.acquire_maintenance.return_value = False
        with self.assertRaisesRegex(ValueError, "server_busy"):
            self.invoke(Mock())
        self.assertEqual(self.proxy.read_bytes(), self.original)
        self.updater.host.release_maintenance.assert_not_called()

    def test_pc_gate_remains_held_on_failed_health(self):
        backup = self.flat / '.momso-upgrade-backup'; backup.mkdir()
        (backup / 'owner').write_text('momso-upgrade-v1')
        (backup / 'mode').write_text('gated')
        self.updater.host.admission._request.return_value = {'maintenance': True, 'quiescent': True}
        def fail(): raise ValueError('proxy_not_running')
        with patch.object(install_worker, 'prepare_verified_baseline'), self.assertRaisesRegex(ValueError, 'proxy_not_running'):
            install_worker.install(self.flat, self.units, self.worker, self.config_path, self.config,
                                   self.updater, 'manifest', 'sig', run=Mock(), gate_already_held=True, health_check=fail)
        self.assertEqual(self.proxy.read_bytes(), self.original)
        self.updater.host.acquire_maintenance.assert_not_called()
        self.updater.host.release_maintenance.assert_not_called()

    def test_unverified_pc_gate_cannot_change_worker(self):
        with self.assertRaises((OSError, ValueError)):
            install_worker.install(self.flat, self.units, self.worker, self.config_path, self.config,
                                   self.updater, 'manifest', 'sig', run=Mock(), gate_already_held=True)
        self.assertEqual(self.proxy.read_bytes(), self.original)
        self.assertFalse(self.config_path.exists())
        self.updater.host.release_maintenance.assert_not_called()

    @unittest.skipIf(os.name == 'nt', 'baseline uses Linux symlinks')
    def test_failed_initial_baseline_is_removed_without_opening_pc_gate(self):
        backup = self.flat / '.momso-upgrade-backup'; backup.mkdir()
        (backup / 'owner').write_text('momso-upgrade-v1')
        (backup / 'mode').write_text('gated')
        self.updater.host.admission._request.return_value = {'maintenance': True, 'quiescent': True}
        root = Path(self.config['updateRoot'])
        def make_baseline(*_args):
            root.mkdir(); (root / 'baseline-fixture').mkdir()
            (root / 'current').symlink_to('baseline-fixture')
            (root / 'installed.json').write_text(json.dumps({'sequence': 2, 'baseline': 'baseline-fixture'}))
        def fail(): raise ValueError('proxy_not_running')
        with patch.object(install_worker, 'prepare_verified_baseline', side_effect=make_baseline), self.assertRaisesRegex(ValueError, 'proxy_not_running'):
            install_worker.install(self.flat, self.units, self.worker, self.config_path, self.config,
                                   self.updater, 'manifest', 'sig', run=Mock(), gate_already_held=True, health_check=fail)
        self.assertFalse((root / 'current').exists())
        self.assertFalse((root / 'installed.json').exists())
        self.assertFalse((root / 'baseline-fixture').exists())
        self.updater.host.release_maintenance.assert_not_called()
