import hashlib
import tarfile
import tempfile
import unittest
from pathlib import Path
from unittest.mock import patch

import prepare_momso_anywhere_bundle as bundle


class Tests(unittest.TestCase):
    def test_cli_uses_recorded_history_without_private_git_objects(self):
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            server = root / 'server'; server.write_bytes(b'fixture-server')
            web = root / 'web'; web.mkdir(); (web / 'index.html').write_text('fixture', encoding='utf-8')
            output = root / 'bundle.tar.gz'
            arguments = ['prepare', '--server', str(server), '--server-sha256', hashlib.sha256(server.read_bytes()).hexdigest(), '--web', str(web), '--output', str(output)]
            with patch('sys.argv', arguments), patch('generate_momso_managed_history.generate', side_effect=AssertionError('private history unavailable')):
                self.assertEqual(bundle.main(), 0)
            with tarfile.open(output) as archive:
                self.assertIn('momso-managed-history.json', archive.getnames())

    def test_bundle_is_reproducible_and_contains_only_expected_files(self):
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            server = root / "server"
            server.write_bytes(b"fixture-server")
            web = root / "web"
            web.mkdir()
            (web / "index.html").write_text("fixture", encoding="utf-8")
            digest = hashlib.sha256(server.read_bytes()).hexdigest()
            first = root / "one.tar.gz"
            second = root / "two.tar.gz"
            self.assertEqual(bundle.prepare(server, digest, web, first), bundle.prepare(server, digest, web, second))
            with tarfile.open(first) as archive:
                expected = {"momso-server", "momso_mobile_proxy.py", "momso_anywhere_vm_proxy.py", "vault-provision.py", "idle-stop.py", "web/index.html", "momso_anywhere_backup.py", "momso_anywhere_backup_agent.py", "momso_anywhere_backup_google.py", "momso_anywhere_vault_identity.py", "momso_anywhere_prestart_restore.py"}
                expected.update({"momso_anywhere_vm_pause.py", "momso_anywhere_pause_ticket.py", "momso_anywhere_pause_guard.py", "server_admission.py"})
                expected.add('momso-updater/momso_anywhere_proxy_files.py')
                expected.add('momso-updater/momso_anywhere_proxy_files.json')
                expected.add('momso-managed-history.json')
                expected.update("momso-updater/" + name for name in ("update.py", "release_manifest.py", "server_admission.py", "systemd_host.py", "data_snapshots.py", "cloud_run_gateway.py", "github_releases.py", "progress_status.py", "layout_migration.py", "install_worker.py", "worker.py", "ensure_gcloud.py"))
                expected.update({"momso-updater/managed_helpers.py", "momso-updater/momso_managed_files.py", "momso-updater/trust_rotation.py", "momso-units/momso-idle-stop.service", "momso-overrides/vault.conf"})
                self.assertEqual(set(archive.getnames()), expected)
            with self.assertRaises(ValueError): bundle.prepare(server, "0" * 64, web, root / "no.tar.gz")


if __name__ == "__main__": unittest.main()
