"""Opt-in local image smoke test; never connects to a Google project."""
import http.client
import json
import os
import subprocess
import tempfile
import time
import unittest
import uuid
from pathlib import Path

from momso_anywhere_public_assets import ASSETS
from prepare_momso_anywhere_image import prepare


@unittest.skipUnless(os.environ.get('MOMSO_TEST_LOCAL_IMAGE') == '1', 'opt-in Docker smoke test')
class ImageSmokeTest(unittest.TestCase):
    def test_anonymous_requests_never_wake(self):
        suffix = uuid.uuid4().hex[:12]
        tag, name = 'momso-s168-test:' + suffix, 'momso-s168-test-' + suffix
        with tempfile.TemporaryDirectory(prefix='momso-s168-image-') as temporary:
            root = Path(temporary)
            web = root / 'web'
            for item in ASSETS:
                path = web / item
                path.parent.mkdir(parents=True, exist_ok=True)
                path.write_bytes(json.dumps({'start_url': '/', 'scope': '/', 'icons': [{'src': '/app-icons/icon-192.png'}]}).encode() if item.endswith('.webmanifest') else b'\x89PNG\r\n\x1a\nfixture')
            prepare(web, root / 'context')
            try:
                subprocess.run(['docker', 'build', '--tag', tag, '--file', str(root / 'context/scripts/momso_anywhere_image/Dockerfile'), str(root / 'context')], check=True, stdout=subprocess.DEVNULL, timeout=300)
                # Observe any wake call without access to metadata or a real VM.
                runner = "import momso_anywhere_gateway as g; g.start_instance=lambda:print('WAKE_CALLED',flush=True); g.http.server.ThreadingHTTPServer(('0.0.0.0',8080),g.Handler).serve_forever()"
                subprocess.run(['docker', 'run', '-d', '--name', name, '-p', '127.0.0.1::8080', '-e', 'MOMSO_PUBLIC_HOST=fixture.run.app', '-e', 'MOMSO_PRIVATE_PROXY_TOKEN=fixture', tag, 'python', '-u', '-c', runner], check=True, stdout=subprocess.DEVNULL, timeout=30)
                mapping = subprocess.check_output(['docker', 'port', name, '8080/tcp'], text=True, timeout=30).strip()
                port = int(mapping.rsplit(':', 1)[1])
                for index in range(200):
                    for attempt in range(30):
                        try:
                            connection = http.client.HTTPConnection('127.0.0.1', port, timeout=2)
                            connection.request('GET', '/__momso/ready' if index % 2 else '/chats', headers={'Host': 'fixture.run.app'})
                            response = connection.getresponse()
                            self.assertEqual(response.status, 200 if index % 2 else 401)
                            response.read(); connection.close()
                            break
                        except (ConnectionError, OSError):
                            if attempt == 29: raise
                            time.sleep(.1)
                logs = subprocess.check_output(['docker', 'logs', name], stderr=subprocess.STDOUT, text=True, timeout=30)
                self.assertNotIn('WAKE_CALLED', logs)
            finally:
                subprocess.run(['docker', 'rm', '-f', name], stdout=subprocess.DEVNULL, stderr=subprocess.DEVNULL, check=False, timeout=30)
                subprocess.run(['docker', 'image', 'rm', tag], stdout=subprocess.DEVNULL, stderr=subprocess.DEVNULL, check=False, timeout=30)


if __name__ == '__main__':
    unittest.main()
