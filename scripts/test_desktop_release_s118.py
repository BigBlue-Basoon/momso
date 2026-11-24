import importlib.util
import json
import os
from pathlib import Path
import shutil
import subprocess
import tempfile
import unittest
from unittest.mock import patch

SCRIPT = Path(__file__).with_name("desktop-release-s118.py")
spec = importlib.util.spec_from_file_location("desktop_release_s118", SCRIPT)
release = importlib.util.module_from_spec(spec)
spec.loader.exec_module(release)
VERSION = json.loads(release.CONFIG.read_text(encoding="utf-8"))["version"]


class DesktopReleaseTests(unittest.TestCase):
    def test_prepared_signer_generates_and_uses_temporary_key(self):
        bundled_node = release.ROOT / 'apps/alpha/desktop/src-tauri/generated/node/node.exe'
        node = str(bundled_node) if bundled_node.is_file() else shutil.which('node')
        packages = release.ROOT / 'node_modules/@tauri-apps'
        if not node or not (packages / 'cli-win32-x64-msvc').is_dir():
            self.skipTest('Windows Node and Tauri packages are not installed')
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory); signer = root / 'signer'; dest = signer / 'node_modules/@tauri-apps'
            dest.mkdir(parents=True)
            shutil.copy2(node, signer / 'node.exe')
            for package in ('cli', 'cli-win32-x64-msvc'):
                shutil.copytree(packages / package, dest / package)
            receipt = {'signerTreeSha256': release.signer_tree_hash(signer)}
            exe, cli = release.resolve_signer(root / 'fixture.exe', receipt)
            key = root / 'fixture.key'; payload = root / 'fixture.bin'; payload.write_bytes(os.urandom(128))
            password = os.urandom(12).hex()
            env = {k: v for k, v in os.environ.items() if not k.startswith('TAURI_SIGNING_')}
            env['TAURI_SIGNING_PRIVATE_KEY_PASSWORD'] = password
            subprocess.run([str(exe), str(cli), 'signer', 'generate', '--ci', '--password', password, '--write-keys', str(key)], env=env, check=True, capture_output=True)
            subprocess.run([str(exe), str(cli), 'signer', 'sign', '--private-key-path', str(key), str(payload)], env=env, check=True, capture_output=True)
            self.assertTrue(Path(str(payload) + '.sig').is_file())

    def test_signer_integrity_rejects_modified_tools(self):
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory); installer = root / 'fixture.exe'; signer = root / 'signer'
            cli = signer / 'node_modules/@tauri-apps/cli/tauri.js'; cli.parent.mkdir(parents=True)
            cli.write_bytes(b'fixture-cli'); (signer / 'node.exe').write_bytes(b'fixture-node')
            receipt = {'signerTreeSha256': release.signer_tree_hash(signer)}
            self.assertEqual(release.resolve_signer(installer, receipt)[1], cli)
            cli.write_bytes(b'altered')
            with self.assertRaisesRegex(ValueError, 'changed'):
                release.resolve_signer(installer, receipt)

    def test_public_commit_rejects_missing_or_bad_source(self):
        with self.assertRaisesRegex(ValueError, "full public source"):
            release.verify_public_commit(Path("fixture"), None)
        with self.assertRaisesRegex(ValueError, "full public source"):
            release.verify_public_commit(Path("fixture"), "abc")

    def test_public_commit_requires_current_main_and_ancestry(self):
        sha, main = "1" * 40, "2" * 40
        def run(args, **_kwargs):
            command = args[3:]
            if command[:2] == ["remote", "get-url"]:
                output = release.PUBLIC_REPOSITORY
            elif command[0] == "fetch":
                output = ""
            elif command[0] == "ls-remote":
                output = main + "\trefs/heads/main"
            elif command[0] == "rev-parse":
                output = main
            elif command[0] == "merge-base":
                self.assertEqual(command[-2:], [sha, main])
                output = ""
            else:
                raise AssertionError(command)
            return subprocess.CompletedProcess(args, 0, output, "")
        with patch.object(release.subprocess, "run", side_effect=run):
            release.verify_public_commit(Path("fixture"), sha)

    def test_public_commit_rejects_changed_main(self):
        def run(args, **_kwargs):
            cmd = args[3:]
            output = release.PUBLIC_REPOSITORY if cmd[0] == 'remote' else ('2' * 40 if cmd[0] == 'ls-remote' else '3' * 40 if cmd[0] == 'rev-parse' else '')
            return subprocess.CompletedProcess(args, 0, output, '')
        with patch.object(release.subprocess, 'run', side_effect=run), self.assertRaisesRegex(ValueError, 'moved'):
            release.verify_public_commit(Path('fixture'), '1' * 40)

    def test_public_commit_rejects_non_ancestor(self):
        def run(args, **_kwargs):
            cmd = args[3:]
            if cmd[0] == 'merge-base':
                raise subprocess.CalledProcessError(1, args)
            output = release.PUBLIC_REPOSITORY if cmd[0] == 'remote' else ('2' * 40 if cmd[0] in ('ls-remote', 'rev-parse') else '')
            return subprocess.CompletedProcess(args, 0, output, '')
        with patch.object(release.subprocess, 'run', side_effect=run), self.assertRaises(subprocess.CalledProcessError):
            release.verify_public_commit(Path('fixture'), '1' * 40)

    @patch.object(release, "check_bundle", return_value=1)
    def test_authorized_current_user_evidence_requires_rendered_screen(self, _inspect):
        with tempfile.TemporaryDirectory() as temp:
            root = Path(temp)
            installer = root / f"momso_{VERSION}_x64-setup.exe"
            installer.write_bytes(b"MZ" + os.urandom(1024))
            terms = root / "terms.txt"
            terms.write_text("fixture-private-term", encoding="utf-8")
            with patch("sys.argv", [str(SCRIPT), "prepare", "--installer", str(installer), "--private-terms-file", str(terms), "--installed-dir", str(root), "--staged-dir", str(root), "--built-exe", str(root / "built.exe")]):
                release.main()
            receipt_path = root / "momso-release-preparation.json"
            receipt = json.loads(receipt_path.read_text(encoding="utf-8"))
            evidence = root / "startup.json"
            value = {"installerSha256": receipt["sha256"], "isolatedUser": False, "authorizedCurrentUser": True,
                     "windowResponsive": True, "firstScreenRendered": False, "uninstalled": True}
            evidence.write_text(json.dumps(value), encoding="utf-8")
            with patch("sys.argv", [str(SCRIPT), "record-startup", "--installer", str(installer), "--private-terms-file", str(terms), "--startup-evidence", str(evidence)]):
                with self.assertRaisesRegex(ValueError, "evidence"):
                    release.main()
                value["firstScreenRendered"] = True
                evidence.write_text(json.dumps(value), encoding="utf-8")
                release.main()
            self.assertEqual(json.loads(receipt_path.read_text(encoding="utf-8"))["startupMode"], "authorized_current_user")

    def record_fixture_startup(self, installer, terms):
        receipt = json.loads((installer.parent / "momso-release-preparation.json").read_text(encoding="utf-8"))
        evidence = installer.parent / "startup.json"
        evidence.write_text(json.dumps({"installerSha256": receipt["sha256"], "isolatedUser": True,
                                        "windowResponsive": True, "uninstalled": True}), encoding="utf-8")
        with patch("sys.argv", [str(SCRIPT), "record-startup", "--installer", str(installer),
                                "--private-terms-file", str(terms), "--startup-evidence", str(evidence)]):
            release.main()
        receipt_path = installer.parent / "momso-release-preparation.json"
        receipt = json.loads(receipt_path.read_text(encoding="utf-8"))
        receipt["sourceCommitSha"] = "1" * 40
        receipt_path.write_text(json.dumps(receipt), encoding="utf-8")

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

    @patch.object(release, "check_bundle", return_value=1)
    def test_prepared_installer_changed_is_rejected_before_signing(self, _inspect):
        with tempfile.TemporaryDirectory() as temp:
            root = Path(temp)
            installer = root / f"momso_{VERSION}_x64-setup.exe"
            installer.write_bytes(b"MZ" + b"x" * 1024)
            key = root / "fake.key"
            key.write_bytes(os.urandom(32))
            terms = root / "terms.txt"
            terms.write_text("fixture-private-term", encoding="utf-8")
            with patch("sys.argv", [str(SCRIPT), "prepare", "--installer", str(installer), "--private-terms-file", str(terms), "--installed-dir", str(root), "--staged-dir", str(root), "--built-exe", str(root / "built.exe")]):
                release.main()
            installer.write_bytes(b"MZ" + b"y" * 1024)
            with patch("sys.argv", [str(SCRIPT), "sign", "--installer", str(installer), "--key-path", str(key), "--private-terms-file", str(terms)]):
                with patch.object(release.subprocess, "run") as signer:
                    with self.assertRaisesRegex(ValueError, "changed"):
                        release.main()
                    signer.assert_not_called()

    @patch.object(release, "check_bundle", return_value=1)
    @patch.object(release, "verify_public_commit")
    @patch.object(release, "resolve_signer", return_value=("node", release.CLI))
    def test_sign_writes_desktop_manifest_without_disclosing_password(self, _signer, _verify, _inspect):
        with tempfile.TemporaryDirectory() as temp:
            root = Path(temp)
            installer = root / f"momso_{VERSION}_x64-setup.exe"
            installer.write_bytes(b"MZ" + b"x" * 1024)
            key = root / "fake.key"
            key.write_bytes(os.urandom(32))
            terms = root / "terms.txt"
            terms.write_text("fixture-private-term", encoding="utf-8")
            with patch("sys.argv", [str(SCRIPT), "prepare", "--installer", str(installer), "--private-terms-file", str(terms), "--installed-dir", str(root), "--staged-dir", str(root), "--built-exe", str(root / "built.exe")]):
                release.main()
            self.record_fixture_startup(installer, terms)
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

    @patch.object(release, "check_bundle", return_value=1)
    @patch.object(release, "verify_public_commit")
    @patch.object(release, "resolve_signer", return_value=("node", release.CLI))
    def test_real_tauri_signer_with_temporary_key(self, _signer, _verify, _inspect):
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
            with patch("sys.argv", [str(SCRIPT), "prepare", "--installer", str(installer), "--private-terms-file", str(terms), "--installed-dir", str(root), "--staged-dir", str(root), "--built-exe", str(root / "built.exe")]):
                release.main()
            self.record_fixture_startup(installer, terms)
            with patch("sys.argv", [str(SCRIPT), "sign", "--installer", str(installer), "--key-path", str(key), "--private-terms-file", str(terms)]):
                with patch.object(release.getpass, "getpass", return_value=password):
                    release.main()
            signature = Path(str(installer) + ".sig").read_text(encoding="utf-8").strip()
            self.assertGreater(len(signature), 32)
            self.assertEqual(json.loads((root / "latest.json").read_text())["platforms"]["windows-x86_64"]["signature"], signature)

    @patch.object(release, "check_bundle", return_value=1)
    def test_signing_requires_product_startup_evidence(self, _inspect):
        with tempfile.TemporaryDirectory() as temp:
            root = Path(temp)
            installer = root / f"momso_{VERSION}_x64-setup.exe"
            installer.write_bytes(b"MZ" + b"x" * 1024)
            terms = root / "terms.txt"
            terms.write_text("fixture-private-term", encoding="utf-8")
            with patch("sys.argv", [str(SCRIPT), "prepare", "--installer", str(installer), "--private-terms-file", str(terms), "--installed-dir", str(root), "--staged-dir", str(root), "--built-exe", str(root / "built.exe")]):
                release.main()
            with patch("sys.argv", [str(SCRIPT), "sign", "--installer", str(installer), "--private-terms-file", str(terms)]):
                with patch.object(release.subprocess, "run") as signer, self.assertRaisesRegex(ValueError, "startup"):
                    release.main()
                signer.assert_not_called()

    @patch.object(release, 'check_bundle', return_value=1)
    def test_signing_rejects_legacy_receipt_without_public_commit(self, _inspect):
        with tempfile.TemporaryDirectory() as temp:
            root = Path(temp)
            installer = root / f'momso_{VERSION}_x64-setup.exe'
            installer.write_bytes(b'MZ' + os.urandom(1024))
            terms = root / 'terms.txt'; terms.write_text('fixture-private-term', encoding='utf8')
            with patch('sys.argv', [str(SCRIPT), 'prepare', '--installer', str(installer), '--private-terms-file', str(terms), '--installed-dir', str(root), '--staged-dir', str(root), '--built-exe', str(root / 'built.exe')]):
                release.main()
            self.record_fixture_startup(installer, terms)
            receipt = root / 'momso-release-preparation.json'
            value = json.loads(receipt.read_text(encoding='utf8')); del value['sourceCommitSha']
            receipt.write_text(json.dumps(value), encoding='utf8')
            with patch('sys.argv', [str(SCRIPT), 'sign', '--installer', str(installer), '--private-terms-file', str(terms)]):
                with self.assertRaisesRegex(ValueError, 'public source'):
                    release.main()

    def test_private_term_and_key_pattern_inside_tree_are_rejected(self):
        with tempfile.TemporaryDirectory() as temp:
            root = Path(temp)
            staged = root / "staged"
            staged.mkdir()
            terms = root / "terms.txt"
            terms.write_text("fixture-private-name", encoding="utf-8")
            (staged / "data.txt").write_text("fixture-private-name", encoding="utf-8")
            with self.assertRaisesRegex(ValueError, "private"):
                release.check_tree(staged, terms)
            (staged / "data.txt").write_bytes(b"ghp_" + b"a" * 25)
            with self.assertRaisesRegex(ValueError, "key pattern"):
                release.check_tree(staged, terms)
            (staged / "data.txt").write_bytes(("ghp_" + "a" * 25).encode("utf-16-le"))
            with self.assertRaisesRegex(ValueError, "key pattern"):
                release.check_tree(staged, terms)

    def test_installed_product_must_match_built_executable(self):
        with tempfile.TemporaryDirectory() as temp:
            root = Path(temp)
            installed, stage = root / "installed", root / "stage"
            (installed / "node").mkdir(parents=True)
            stage.mkdir()
            (installed / "node/node.exe").write_bytes(b"node")
            built = root / "built.exe"
            built.write_bytes(b"fixture__TAURI_BUNDLE_TYPE_VAR_UNK")
            (installed / "momso-desktop.exe").write_bytes(b"fixture__TAURI_BUNDLE_TYPE_VAR_NSS")
            (stage / "source").write_bytes(b"safe")
            (stage / "node.exe").write_bytes(b"node")
            terms = root / "terms.txt"
            terms.write_text("unrelated-fixture-private-term", encoding="utf-8")
            self.assertEqual(release.check_bundle(installed, [stage], terms, built), 4)
            (installed / "momso-desktop.exe").write_bytes(b"wrong executable")
            with self.assertRaisesRegex(ValueError, "does not match"):
                release.check_bundle(installed, [stage], terms, built)
            (installed / "momso-desktop.exe").write_bytes(b"fixture__TAURI_BUNDLE_TYPE_VAR_NSS")
            (installed / "node/node.exe").write_bytes(b"altered")
            with self.assertRaisesRegex(ValueError, "Node does not match"):
                release.check_bundle(installed, [stage], terms, built)


if __name__ == "__main__":
    unittest.main()
