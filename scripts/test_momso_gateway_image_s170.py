"""Opt-in local image plus VM proxy smoke; no Google credentials or account."""
import json
import os
import subprocess
import tempfile
import unittest
import uuid
from pathlib import Path

from momso_anywhere_public_assets import ASSETS
from prepare_momso_anywhere_image import prepare


@unittest.skipUnless(os.environ.get('MOMSO_TEST_LOCAL_IMAGE') == '1', 'opt-in Docker smoke test')
class ImageTest(unittest.TestCase):
    def test_512m_image_forwards_large_chunked_streams(self):
        tag = 'momso-s170-test:' + uuid.uuid4().hex[:12]
        with tempfile.TemporaryDirectory(prefix='momso-s170-image-') as temporary:
            root = Path(temporary)
            web = root / 'web'
            for item in ASSETS:
                path = web / item
                path.parent.mkdir(parents=True, exist_ok=True)
                path.write_bytes(json.dumps({'start_url': '/', 'scope': '/', 'icons': [{'src': '/app-icons/icon-192.png'}]}).encode() if item.endswith('.webmanifest') else b'\x89PNG\r\n\x1a\nfixture')
            prepare(web, root / 'context')
            try:
                subprocess.run(['docker', 'build', '--tag', tag, '--file', str(root / 'context/scripts/momso_anywhere_image/Dockerfile'), str(root / 'context')], check=True, stdout=subprocess.DEVNULL, timeout=300)
                export = os.environ.get('MOMSO_EXPORT_ROOTFS')
                if export:
                    container = subprocess.check_output(['docker', 'create', tag], text=True, timeout=30).strip()
                    try:
                        subprocess.run(['docker', 'export', '--output', export, container], check=True, timeout=300)
                    finally:
                        subprocess.run(['docker', 'rm', container], stdout=subprocess.DEVNULL, stderr=subprocess.DEVNULL, check=False, timeout=30)
                scripts = str(Path(__file__).resolve().parent)
                code = "import sys;sys.path[:0]=['/app','/source'];import momso_gateway_s170_measure as t;t.main()"
                output = subprocess.check_output(['docker', 'run', '--rm', '--memory=512m', '-v', scripts + ':/source:ro', tag, 'python', '-u', '-c', code, '--duration', '0', '--chunks'], text=True, timeout=300)
                entries = [json.loads(line) for line in output.splitlines() if line.startswith('{')]
                chunks = [value['chunked_mib'] for value in entries if 'chunked_mib' in value]
                self.assertEqual(chunks, [10, 40, 200, 1024])
                self.assertTrue(entries[-1]['finished'])
                print(output, flush=True)
            finally:
                subprocess.run(['docker', 'image', 'rm', tag], stdout=subprocess.DEVNULL, stderr=subprocess.DEVNULL, check=False, timeout=30)


if __name__ == '__main__':
    unittest.main()
