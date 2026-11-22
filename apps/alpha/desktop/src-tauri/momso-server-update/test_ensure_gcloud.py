import io
from pathlib import Path
import tempfile
import unittest
from types import SimpleNamespace
from unittest.mock import patch

import ensure_gcloud


class GcloudDependencyTests(unittest.TestCase):
    def test_existing_gcloud_does_not_install_packages(self):
        with patch.object(ensure_gcloud.shutil, "which", return_value="/usr/bin/gcloud"), patch.object(ensure_gcloud.subprocess, "run") as run:
            ensure_gcloud.ensure()
            run.assert_not_called()

    def test_missing_gcloud_uses_signed_debian_repository(self):
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            def fake_path(value):
                return root / ("keyring.gpg" if value.endswith(".gpg") else "source.list")
            with patch.object(ensure_gcloud, "Path", side_effect=fake_path), \
                 patch.object(ensure_gcloud.shutil, "which", side_effect=[None, "/usr/bin/gcloud"]), \
                 patch.object(ensure_gcloud.urllib.request, "urlopen", return_value=io.BytesIO(b"fixture-key")), \
                 patch.object(ensure_gcloud.subprocess, "run", return_value=SimpleNamespace(stdout=b"fixture-keyring")) as run:
                ensure_gcloud.ensure()
            self.assertEqual((root / "keyring.gpg").read_bytes(), b"fixture-keyring")
            self.assertIn("signed-by=/usr/share/keyrings/cloud.google.gpg", (root / "source.list").read_text())
            self.assertIn(("apt-get", "install", "-y", "-qq", "google-cloud-cli"), [call.args[0] for call in run.call_args_list])
