import json
import subprocess
import tempfile
import unittest
from pathlib import Path
from unittest.mock import patch

from momso_anywhere_rehearsal_s147_real import RealAdapter


class Tests(unittest.TestCase):
    def test_adapter_uses_installer_and_preserves_uncertain_inventory(self):
        class Provider:
            install_id = 'a' * 12
            def ssh(self, _code): return b'a' * 64
            def run(self, *_args, **_kwargs): return []
        with tempfile.TemporaryDirectory() as directory:
            state = Path(directory) / 'state.json'
            state.write_text(json.dumps({'restore_result': {'restored': True}}))
            calls = []
            def execute(command, **_kwargs):
                calls.append(command)
                output = b'{"outcome":"deleted_unverified","unverified":["bucket"]}\n' if '--discard-data' in command else b''
                return subprocess.CompletedProcess(command, 0, stdout=output, stderr=b'')
            adapter = RealAdapter(Provider(), state, ['installer'], ['destroy'], run=execute)
            self.assertTrue(adapter.install('a' * 12))
            self.assertEqual(adapter.write_marker('a' * 12), 'a' * 64)
            self.assertTrue(adapter.pause('a' * 12))
            self.assertTrue(adapter.reinstall('a' * 12))
            self.assertEqual(adapter.read_marker('a' * 12), 'a' * 64)
            self.assertTrue(adapter.cleanup('a' * 12))
            self.assertIn('--discard-data', calls[-1])
            with patch('momso_anywhere_rehearsal_s117a.collect_inventory', return_value=({'bucket': {'complete': False, 'items': []}}, [])):
                self.assertEqual(adapter.inventory('a' * 12)['unverified'], ['bucket'])
                self.assertFalse(adapter.inventory('a' * 12)['complete'])
            self.assertFalse(adapter.install('b' * 12))


if __name__ == '__main__': unittest.main()
