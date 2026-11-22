import hashlib
import tarfile
import tempfile
import unittest
from pathlib import Path

import prepare_momso_anywhere_bundle as bundle


class Tests(unittest.TestCase):
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
                self.assertEqual(set(archive.getnames()), {"momso-server", "momso_mobile_proxy.py", "momso_anywhere_vm_proxy.py", "vault-provision.py", "idle-stop.py", "web/index.html", "momso_anywhere_backup.py", "momso_anywhere_backup_agent.py", "momso_anywhere_backup_google.py", "momso_anywhere_vault_identity.py"})
            with self.assertRaises(ValueError): bundle.prepare(server, "0" * 64, web, root / "no.tar.gz")


if __name__ == "__main__": unittest.main()
