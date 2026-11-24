import base64
import json
import tempfile
import unittest
from pathlib import Path
from unittest.mock import patch

import momso_anywhere_public_assets as assets
import momso_release_keys as keys
import prepare_momso_anywhere_static_site as site


class StaticSiteTests(unittest.TestCase):
    def setUp(self):
        self.tmp = tempfile.TemporaryDirectory()
        self.addCleanup(self.tmp.cleanup)
        root = Path(self.tmp.name)
        self.root = root
        login = root / 'login'; login.mkdir()
        (login / 'index.html').write_text('<div id="google-button"><button id="google-login" type="button"></button></div><p id="error" role="alert"></p><script type="module" src="./boot.mjs"></script>', encoding='utf-8')
        (login / 'login.css').write_text('/* fixture */', encoding='utf-8')
        web = root / 'web'; web.mkdir()
        for name in assets.ASSETS:
            path = web / name; path.parent.mkdir(parents=True, exist_ok=True)
            path.write_bytes(json.dumps({'start_url': '/', 'scope': '/', 'icons': [{'src': '/app-icons/icon-192.png'}]}).encode() if name.endswith('.webmanifest') else b'\x89PNG\r\n\x1a\nfixture')
        dist = root / 'dist'; (dist / 'assets').mkdir(parents=True)
        (dist / 'index.html').write_text('<script type="module" src="/start/assets/app.js"></script>', encoding='utf-8')
        (dist / 'assets/app.js').write_text('resumeAfterRedirect', encoding='utf-8')
        public = root / 'production.pem'
        public.write_text('-----BEGIN PUBLIC KEY-----\n' + base64.b64encode(keys.PREFIX + b'\x11' * 32).decode() + '\n-----END PUBLIC KEY-----\n')
        for name, value in [('PRODUCTION_KEY', public), ('DEVELOPMENT_KEY', root / 'absent.pem')]:
            handle = patch.object(keys, name, value); handle.start(); self.addCleanup(handle.stop)
        self.options = dict(output=root / 'output', login_page=login, web=web, start_dist=dist,
                            client_id='fixture.apps.googleusercontent.com', manifest_url='https://example.test/release.json',
                            public_key=public, login_origin='https://example.test')

    def test_combines_static_routes_and_preserves_login_screen(self):
        site.prepare(**self.options)
        root = self.options['output']
        self.assertEqual((root / 'index.html').read_bytes(), (self.options['login_page'] / 'index.html').read_bytes())
        self.assertEqual((root / '_redirects').read_text(), '/start /start/ 308\n')
        self.assertIn('Referrer-Policy: strict-origin', (root / '_headers').read_text())
        self.assertIn('Content-Security-Policy:', (root / '_headers').read_text())
        self.assertTrue((root / '404.html').is_file())
        self.assertEqual(json.loads((root / 'start/config.json').read_text())['release']['channel'], 'production')
        self.assertTrue((root / 'start/momso_anywhere_start.mjs').is_file())

    def test_rejects_dev_key_and_leaves_no_partial_output(self):
        dev = self.root / 'dev.pem'
        dev.write_text('-----BEGIN PUBLIC KEY-----\n' + base64.b64encode(keys.PREFIX + b'\x22' * 32).decode() + '\n-----END PUBLIC KEY-----\n')
        with self.assertRaises(ValueError):
            site.prepare(**{**self.options, 'public_key': dev})
        self.assertFalse(self.options['output'].exists())

    def test_rejects_host_configuration_in_source(self):
        (self.options['login_page'] / '_worker.js').write_text('fixture')
        with self.assertRaisesRegex(ValueError, 'site_overlap'):
            site.prepare(**self.options)
        self.assertFalse(self.options['output'].exists())


if __name__ == '__main__':
    unittest.main()
