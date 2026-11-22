"""Offline checks for the updater test's release-runtime staging."""
import hashlib
import importlib.util
import io
from pathlib import Path
import tempfile
import unittest
from unittest.mock import patch
import zipfile

SCRIPT = Path(__file__).with_name('test-desktop-updater-s69.py')
spec = importlib.util.spec_from_file_location('s69_fixture', SCRIPT)
fixture = importlib.util.module_from_spec(spec)
spec.loader.exec_module(fixture)


def archive(entries):
    output = io.BytesIO()
    with zipfile.ZipFile(output, 'w') as z:
        for path, data in entries.items():
            z.writestr('node-v22.23.5-win-x64/' + path, data)
    return output.getvalue()


class NodeStageTests(unittest.TestCase):
    def run_stage(self, content, destination, digest=None):
        with patch.object(fixture.urllib.request, 'urlopen', return_value=io.BytesIO(content)), patch.object(fixture, 'NODE_SHA256', digest or hashlib.sha256(content).hexdigest()):
            fixture.stage_node(destination)

    def test_stages_required_runtime_and_omits_npm_docs(self):
        content = archive({'node.exe': b'node', 'node_modules/npm/bin/npm-cli.js': b'npm', 'node_modules/npm/docs/readme.txt': b'docs'})
        with tempfile.TemporaryDirectory() as root:
            dest = Path(root) / 'node'
            self.run_stage(content, dest)
            self.assertEqual((dest / 'node.exe').read_bytes(), b'node')
            self.assertFalse((dest / 'node_modules/npm/docs/readme.txt').exists())

    def test_rejects_traversal_even_with_matching_hash(self):
        content = archive({'../outside': b'escape'})
        with tempfile.TemporaryDirectory() as root:
            with self.assertRaisesRegex(RuntimeError, 'unsafe Node archive entry'):
                self.run_stage(content, Path(root) / 'node')
            self.assertFalse((Path(root) / 'outside').exists())

    def test_rejects_checksum_mismatch(self):
        content = archive({'node.exe': b'node'})
        with tempfile.TemporaryDirectory() as root:
            with self.assertRaisesRegex(RuntimeError, 'checksum mismatch'):
                self.run_stage(content, Path(root) / 'node', '0' * 64)


if __name__ == '__main__':
    unittest.main()
