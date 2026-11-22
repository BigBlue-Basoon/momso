import json
import tempfile
import unittest
from pathlib import Path

import prepare_momso_anywhere_login_site as bundle
from momso_anywhere_public_assets import ASSETS


class LoginBundleTests(unittest.TestCase):
    def setUp(self):
        self.tmp = tempfile.TemporaryDirectory()
        self.addCleanup(self.tmp.cleanup)
        self.root = Path(self.tmp.name)
        self.client = "fixture.apps.googleusercontent.com"

    def test_default_keeps_existing_login_page(self):
        output = self.root / "out"
        bundle.prepare(output, self.client)
        self.assertTrue((output / "index.html").is_file())
        self.assertTrue((output / "momso_anywhere_login.mjs").is_file())
        self.assertEqual(json.loads((output / "config.json").read_text()), {"clientId": self.client})

    def test_custom_page_requires_contract_and_boot(self):
        source = self.root / "design"
        source.mkdir()
        (source / "index.html").write_text('<div id="google-button"><button id="google-login" type="button"></button></div><p id="error" role="alert"></p><script type="module" src="./boot.mjs"></script>', encoding="utf-8")
        (source / "login.css").write_text("body {}", encoding="utf-8")
        output = self.root / "out"
        with self.assertRaises(ValueError): bundle.prepare(output, self.client, source)
        self.assertFalse(output.exists())
        web = self.root / "web"
        web.mkdir()
        for name in ASSETS:
            path = web / name
            path.parent.mkdir(parents=True, exist_ok=True)
            if name.endswith('.png'):
                path.write_bytes(b'\x89PNG\r\n\x1a\nfixture')
            else:
                path.write_text(json.dumps({'name': 'momso', 'start_url': '/', 'scope': '/', 'icons': [{'src': '/app-icons/icon-192.png'}]}), encoding='utf-8')
        boot = self.root / "boot.mjs"
        boot.write_text("export {};", encoding="utf-8")
        bundle.prepare(output, self.client, source, boot, web)
        self.assertEqual((output / "boot.mjs").read_text(), "export {};")
        self.assertEqual((output / "login.css").read_text(), "body {}")
        for name in ASSETS:
            self.assertEqual((output / name).read_bytes(), (web / name).read_bytes())
        with self.assertRaises(ValueError): bundle.prepare(output, self.client, source, boot, web)
        default_boot_output = self.root / "default-boot"
        bundle.prepare(default_boot_output, self.client, source, web=web)
        expected_boot = Path(bundle.__file__).with_name("momso_anywhere_login_boot.mjs")
        self.assertEqual((default_boot_output / "boot.mjs").read_bytes(), expected_boot.read_bytes())
        self.assertNotIn("loginReturnUrl", json.loads((default_boot_output / "config.json").read_text()))

    def test_invalid_page_never_creates_output(self):
        source = self.root / "design"
        source.mkdir()
        (source / "index.html").write_text("fixture", encoding="utf-8")
        (source / "login.css").write_text("", encoding="utf-8")
        with self.assertRaises(ValueError): bundle.prepare(self.root / "out", self.client, source)
        self.assertFalse((self.root / "out").exists())


if __name__ == "__main__":
    unittest.main()
