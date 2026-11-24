import importlib.util
import json
import os
from pathlib import Path
import subprocess
import tempfile
import unittest
from unittest.mock import patch
import zipfile

SCRIPT = Path(__file__).with_name("desktop-release-s118.py")
spec = importlib.util.spec_from_file_location("desktop_release_s118", SCRIPT)
release = importlib.util.module_from_spec(spec)
spec.loader.exec_module(release)
VERSION = json.loads(release.CONFIG.read_text(encoding="utf-8"))["version"]


class DesktopReleaseTests(unittest.TestCase):
    def test_private_terms_and_windows_executable(self):
        with tempfile.TemporaryDirectory() as temp:
            root = Path(temp)
            installer = root / f"momso_{VERSION}_x64-setup.exe"
            installer.write_bytes(b"MZ" + b"x" * 1024)
            self.assertEqual(len(release.check_installer(installer, VERSION)), 64)
            terms = root / "private.txt"
            terms.write_text("fixture-private-value", encoding="utf-8")
            installer.write_bytes(b"MZ" + b"x" * 1024 + b"fixture-private-value")
            with self.assertRaisesRegex(ValueError, "private"):
                release.check_installer(installer, VERSION, terms)
            installer.write_bytes(b"not-a-windows-executable")
            with self.assertRaisesRegex(ValueError, "Windows executable"):
                release.check_installer(installer, VERSION)

    @patch.object(release, "check_bundle")
    def test_prepared_installer_changed_is_rejected_before_signing(self, _inspect):
        with tempfile.TemporaryDirectory() as temp:
            root = Path(temp)
            installer = root / f"momso_{VERSION}_x64-setup.exe"
            installer.write_bytes(b"MZ" + b"x" * 1024)
            key = root / "fake.key"
            key.write_bytes(os.urandom(32))
            terms = root / "terms.txt"
            terms.write_text("fixture-private-term", encoding="utf-8")
            with patch("sys.argv", [str(SCRIPT), "prepare", "--installer", str(installer), "--private-terms-file", str(terms)]):
                release.main()
            installer.write_bytes(b"MZ" + b"y" * 1024)
            with patch("sys.argv", [str(SCRIPT), "sign", "--installer", str(installer), "--key-path", str(key), "--private-terms-file", str(terms)]):
                with patch.object(release.subprocess, "run") as signer:
                    with self.assertRaisesRegex(ValueError, "changed"):
                        release.main()
                    signer.assert_not_called()

    @patch.object(release, "check_bundle")
    def test_sign_writes_desktop_manifest_without_disclosing_password(self, _inspect):
        with tempfile.TemporaryDirectory() as temp:
            root = Path(temp)
            installer = root / f"momso_{VERSION}_x64-setup.exe"
            installer.write_bytes(b"MZ" + b"x" * 1024)
            key = root / "fake.key"
            key.write_bytes(os.urandom(32))
            terms = root / "terms.txt"
            terms.write_text("fixture-private-term", encoding="utf-8")
            with patch("sys.argv", [str(SCRIPT), "prepare", "--installer", str(installer), "--private-terms-file", str(terms)]):
                release.main()
            def fake_sign(*_args, **kwargs):
                self.assertEqual(kwargs["env"]["TAURI_SIGNING_PRIVATE_KEY_PASSWORD"], "fixture-password")
                Path(str(installer) + ".sig").write_text("fixture-signature", encoding="utf-8")
            with patch("sys.argv", [str(SCRIPT), "sign", "--installer", str(installer), "--key-path", str(key), "--private-terms-file", str(terms)]):
                with patch.object(release.getpass, "getpass", return_value="fixture-password"), patch.object(release.subprocess, "run", side_effect=fake_sign):
                    release.main()
            manifest = json.loads((root / "latest.json").read_text(encoding="utf-8"))
            self.assertEqual(manifest["platforms"]["windows-x86_64"]["signature"], "fixture-signature")
            self.assertTrue(manifest["platforms"]["windows-x86_64"]["url"].endswith(f"/v{VERSION}/" + installer.name))
            self.assertNotIn("fixture-password", (root / "latest.json").read_text(encoding="utf-8"))

    @patch.object(release, "check_bundle")
    def test_real_tauri_signer_with_temporary_key(self, _inspect):
        if not release.CLI.is_file():
            self.skipTest("Tauri CLI is not installed")
        with tempfile.TemporaryDirectory() as temp:
            root = Path(temp)
            installer = root / f"momso_{VERSION}_x64-setup.exe"
            installer.write_bytes(b"MZ" + os.urandom(1024))
            key = root / "fixture.key"
            terms = root / "terms.txt"
            terms.write_text("fixture-private-term", encoding="utf-8")
            password = os.urandom(12).hex()
            env = {k: v for k, v in os.environ.items() if not k.startswith("TAURI_SIGNING_")}
            env["TAURI_SIGNING_PRIVATE_KEY_PASSWORD"] = password
            subprocess.run(["node", str(release.CLI), "signer", "generate", "--ci", "--password", password, "--write-keys", str(key)],
                           env=env, check=True, capture_output=True)
            with patch("sys.argv", [str(SCRIPT), "prepare", "--installer", str(installer), "--private-terms-file", str(terms)]):
                release.main()
            with patch("sys.argv", [str(SCRIPT), "sign", "--installer", str(installer), "--key-path", str(key), "--private-terms-file", str(terms)]):
                with patch.object(release.getpass, "getpass", return_value=password):
                    release.main()
            signature = Path(str(installer) + ".sig").read_text(encoding="utf-8").strip()
            self.assertGreater(len(signature), 32)
            self.assertEqual(json.loads((root / "latest.json").read_text())["platforms"]["windows-x86_64"]["signature"], signature)

    def test_private_term_inside_compressed_bundle_is_rejected(self):
        if not (release.ROOT / "node_modules/.bin/7z.exe").is_file():
            self.skipTest("7-Zip is not installed")
        with tempfile.TemporaryDirectory() as temp:
            root = Path(temp)
            archive = root / "fixture.zip"
            terms = root / "terms.txt"
            terms.write_text("fixture-private-name", encoding="utf-8")
            with zipfile.ZipFile(archive, "w", compression=zipfile.ZIP_DEFLATED) as zipped:
                zipped.writestr("data.txt", "fixture-private-name")
            self.assertNotIn(b"fixture-private-name", archive.read_bytes())
            with self.assertRaisesRegex(ValueError, "private"):
                release.check_bundle(archive, terms)


if __name__ == "__main__":
    unittest.main()
