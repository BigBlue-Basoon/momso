import json
import base64
import tempfile
import unittest
from pathlib import Path
from unittest.mock import patch

import prepare_momso_anywhere_start_site as site


class StartSiteTests(unittest.TestCase):
    def setUp(self):
        self.tmp = tempfile.TemporaryDirectory()
        self.addCleanup(self.tmp.cleanup)
        self.root = Path(self.tmp.name)
        self.dist = self.root / "dist"
        (self.dist / "assets").mkdir(parents=True)
        (self.dist / "assets/app.js").write_text("fixture resumeAfterRedirect", encoding="utf-8")
        (self.dist / "index.html").write_text('<script type="module" src="/start/assets/app.js"></script>', encoding="utf-8")
        key = self.root / 'public.pem'
        key.write_text('-----BEGIN PUBLIC KEY-----\n' + base64.b64encode(bytes.fromhex('302a300506032b6570032100') + b'\x11' * 32).decode() + '\n-----END PUBLIC KEY-----\n')
        for name, value in [('PRODUCTION_KEY', key), ('DEVELOPMENT_KEY', self.root / 'dev-public.pem')]:
            p = patch.object(site.momso_release_keys, name, value); p.start(); self.addCleanup(p.stop)
        self.args = ("fixture.apps.googleusercontent.com", "https://example.test/release.json", key, "https://login.example.test")

    def test_packages_modules_config_and_screen(self):
        output = self.root / "output"
        site.prepare(self.dist, output, *self.args)
        self.assertEqual(json.loads((output / "config.json").read_text(encoding="utf-8"))["release"]["manifestUrl"], "https://example.test/release.json")
        self.assertTrue(all((output / name).is_file() for name in site.MODULES))
        html = (output / "index.html").read_text(encoding="utf-8")
        self.assertLess(html.index("momso_anywhere_oauth_capture.mjs"), html.index("/start/assets/app.js"))
        self.assertEqual((output / "assets/app.js").read_text(encoding="utf-8"), "fixture resumeAfterRedirect")
        self.assertNotIn("access_token", (output / "config.json").read_text(encoding="utf-8"))

    def test_rejects_wrong_base_overlap_and_bad_digest(self):
        (self.dist / "index.html").write_text('<script type="module" src="/assets/app.js"></script>', encoding="utf-8")
        with self.assertRaisesRegex(ValueError, "build_base"):
            site.prepare(self.dist, self.root / "output", *self.args)
        (self.dist / "index.html").write_text('<script type="module" src="/start/assets/app.js"></script>', encoding="utf-8")
        with self.assertRaisesRegex(ValueError, "invalid_paths"):
            site.prepare(self.dist, self.dist / "output", *self.args)
        with self.assertRaisesRegex(ValueError, "invalid_release"):
            site.prepare(self.dist, self.root / "output", self.args[0], self.args[1], "bad", *self.args[3:])
        with self.assertRaisesRegex(ValueError, "invalid_release"):
            site.prepare(self.dist, self.root / "output", self.args[0], "https://example.test:8443/file", *self.args[2:])

    def test_rejects_screen_without_redirect_resume(self):
        (self.dist / "assets/app.js").write_text("old screen", encoding="utf-8")
        with self.assertRaisesRegex(ValueError, "screen_resume_missing"):
            site.prepare(self.dist, self.root / "output", *self.args)

    def test_rejects_async_or_external_script_before_token_capture(self):
        for html in ('<script type="module" async src="/start/assets/app.js"></script>',
                     '<script type="module" src="https://example.test/app.js"></script>'):
            with self.subTest(html=html):
                (self.dist / "index.html").write_text(html, encoding="utf-8")
                with self.assertRaisesRegex(ValueError, "unsafe_build_script"):
                    site.prepare(self.dist, self.root / "output", *self.args)

    def test_production_rejects_development_key_even_when_copied(self):
        dev = site.momso_release_keys.DEVELOPMENT_KEY
        dev.write_text('-----BEGIN PUBLIC KEY-----\n' + base64.b64encode(bytes.fromhex('302a300506032b6570032100') + b'\x22' * 32).decode() + '\n-----END PUBLIC KEY-----\n')
        copied = self.root / 'copied.pem'; copied.write_bytes(dev.read_bytes())
        with self.assertRaisesRegex(ValueError, 'invalid_release'):
            site.prepare(self.dist, self.root / 'production', self.args[0], self.args[1], copied, self.args[3])
        with patch.object(site.momso_release_keys, 'PRODUCTION_KEY', copied):
            with self.assertRaisesRegex(ValueError, 'invalid_release'):
                site.prepare(self.dist, self.root / 'wrong-pin', self.args[0], self.args[1], copied, self.args[3])
        output = self.root / 'development'
        site.prepare(self.dist, output, self.args[0], self.args[1], copied, self.args[3], channel='development')
        self.assertEqual(json.loads((output / 'config.json').read_text())['release']['channel'], 'development')

    def test_production_fails_when_no_pinned_key_exists(self):
        with patch.object(site.momso_release_keys, 'PRODUCTION_KEY', self.root / 'absent.pem'):
            with self.assertRaisesRegex(ValueError, 'invalid_release'):
                site.prepare(self.dist, self.root / 'output', *self.args)

    def test_production_rejects_development_public_key_inside_screen_assets(self):
        dev = site.momso_release_keys.DEVELOPMENT_KEY
        data = bytes.fromhex('302a300506032b6570032100') + b'\x22' * 32
        encoded = base64.b64encode(data).decode()
        dev.write_text('-----BEGIN PUBLIC KEY-----\n' + encoded + '\n-----END PUBLIC KEY-----\n')
        (self.dist / 'assets/app.js').write_text('resumeAfterRedirect ' + encoded, encoding='utf-8')
        with self.assertRaisesRegex(ValueError, 'development_key_in_build'):
            site.prepare(self.dist, self.root / 'output', *self.args)
        self.assertFalse((self.root / 'output').exists())


if __name__ == "__main__":
    unittest.main()
