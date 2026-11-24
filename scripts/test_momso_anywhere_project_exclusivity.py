import io
import json
import tempfile
import unittest
from contextlib import redirect_stdout
from pathlib import Path
from unittest.mock import patch

import momso_server_install
from momso_anywhere_project_exclusivity import inspect_project


class Provider:
    project = 'fixture-project'
    install_id = 'a' * 12
    def __init__(self, denied=False): self.denied = denied; self.calls = []
    def run(self, *args, **kwargs):
        self.calls.append(args)
        if args[:2] == ('asset', 'search-all-resources'):
            if self.denied: raise RuntimeError('fixture')
            return [{'assetType': 'compute.googleapis.com/Instance', 'name': '//compute.googleapis.com/projects/fixture-project/zones/z/instances/another-vm'}]
        if args[:2] == ('services', 'list'):
            return [{'config': {'name': 'compute.googleapis.com'}}]
        raise AssertionError(args)


class Tests(unittest.TestCase):
    def test_read_only_pc_inventory_finds_foreign_vm(self):
        p = Provider(); value = inspect_project(p)
        self.assertEqual(value['status'], 'not_exclusive')
        self.assertEqual(value['checks'], {'assets': 'observed', 'services': 'observed'})
        self.assertIn('compute.googleapis.com/Instance', value['observedTypes'])
        self.assertTrue(all(item not in ('delete', 'create', 'enable', 'disable') for call in p.calls for item in call))

    def test_denied_assets_remain_unverified(self):
        value = inspect_project(Provider(denied=True))
        self.assertEqual(value['status'], 'unverified')
        self.assertEqual(value['checks']['assets'], 'unverified')
        self.assertEqual(value['checks']['services'], 'observed')

    def test_pc_command_requires_state_and_reads_without_writes(self):
        with tempfile.TemporaryDirectory() as directory:
            state = Path(directory) / 'state.json'
            state.write_text(json.dumps({'version': 1, 'id': 'a' * 12, 'account': 'fixture@example.test', 'project': 'fixture-project', 'steps': {}}), encoding='utf-8')
            before = state.read_bytes(); provider = Provider()
            with patch.object(momso_server_install, 'GoogleProvider', return_value=provider), redirect_stdout(io.StringIO()) as output:
                code = momso_server_install.main(['project-exclusivity', '--account', 'fixture@example.test', '--project', 'fixture-project', '--state', str(state)])
            self.assertEqual(code, 0)
            self.assertEqual(json.loads(output.getvalue())['status'], 'not_exclusive')
            self.assertEqual(state.read_bytes(), before)
            self.assertEqual([item.name for item in Path(directory).iterdir()], ['state.json'])


if __name__ == '__main__': unittest.main()
