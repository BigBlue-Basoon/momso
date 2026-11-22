"""Verify the supplied legacy VM fingerprints without reading its private data."""
import ast
import base64
import hashlib
from pathlib import Path
import tempfile
import unittest
from unittest.mock import patch

import momso_anywhere_install
import momso_anywhere_vm_services
import momso_managed_history
import momso_server_install
from test_momso_anywhere_install import Provider
from test_momso_anywhere_vm_services import FakeProvider


def digest(value):
    return hashlib.sha256(value).hexdigest()


class ActualFingerprintTests(unittest.TestCase):
    def test_legacy_files_are_known_history(self):
        expected = {
            'idle-stop.py': 'd02789b88953a03ba6801b53d9fbcbed6c4b9287f22724fc49e79d77073db3d2',
            'momso_anywhere_backup.py': '99283e2bc26dd78dc189297b0c5add8faa3206e1d63ce60e9808da1c59d4c6d8',
            'momso_anywhere_backup_agent.py': '143541d544f08cfbfd2f446d04406260530429138b90c05272ffa5e8e1be94b1',
            'momso_anywhere_backup_google.py': '3c1f96eded9f69acfba0decaf71c7cbde4a6b8cf181b506e27ed9866481e5329',
            'momso_anywhere_vault_identity.py': 'f4ad593ba2ea5ae990b1e21f912f7a65bff4488d4eab571652fb27009e436580',
            'momso_anywhere_vm_proxy.py': '5bb86c2da36236b7a424c17dc8b5aeda19524cbf08cff2dc07d6b4ff74725ab7',
            'momso_mobile_proxy.py': '4778c397e7f85e204258f506d8177366dba91655de5e152c2f67a45e1d1844eb',
            'vault-provision.py': '209253db86c2c17f822448419b32fbd861d0ca8e8759668391b490b80e98e09e',
        }
        history = momso_managed_history.load()
        for name, sha in expected.items():
            groups = [history[group] for group in ('files', 'worker', 'proxy')]
            self.assertTrue(any(sha in group.get(name, []) for group in groups), name)

    def test_static_generated_files_match_supplied_fingerprints(self):
        provider = FakeProvider()
        momso_anywhere_vm_services.install_scripts(provider, 'fixture-secret', False, {'bucket': 'fixture', 'key': 'fixture'})
        tree = ast.parse(provider.payloads[-1])
        expected = {
            'helper': 'e96f079f3c79f6e1b167843f9a8934d16fe9e72bc4e103d9a3f23438f877e205',
            'vault': '7d3f61c13ac543c274e80f636495036069822511574d437dc7bbad61e522e303',
            'idle': '3d4d8144faa7bd622a5cc396b941b1c15a60463b1c1da5e8cd9d51964b1b568f',
        }
        actual = {node.targets[0].id: digest(node.value.value.encode()) for node in ast.walk(tree)
                  if isinstance(node, ast.Assign) and len(node.targets) == 1 and isinstance(node.targets[0], ast.Name)
                  and node.targets[0].id in expected and isinstance(node.value, ast.Constant) and isinstance(node.value.value, str)}
        self.assertEqual(actual, expected)

    def test_proxy_and_server_units_match_supplied_fingerprints(self):
        proxy = Provider()
        momso_anywhere_install.install_private_proxy(proxy)
        tree = ast.parse(proxy.ssh_calls[0][0])
        encoded = next(node.args[0].value for node in ast.walk(tree) if isinstance(node, ast.Call) and isinstance(node.func, ast.Attribute) and node.func.attr == 'b64decode')
        inner = ast.parse(base64.b64decode(encoded))
        unit = next(node.value for node in ast.walk(inner) if isinstance(node, ast.Assign) and any(isinstance(t, ast.Name) and t.id == 'unit' for t in node.targets))
        rendered = eval(compile(ast.Expression(unit), '<fixture>', 'eval'), {'base': '/opt/momso'})
        self.assertEqual(digest(rendered.encode()), '4e4dd7ef91d1bb6dab6bfa63ff96e8cc6c01dcbd421c4cbe129a213e1ab13a28')
        with tempfile.TemporaryDirectory() as directory, patch.object(momso_server_install.shutil, 'which', return_value='gcloud'):
            root = Path(directory); artifact = root / 'server'; artifact.write_bytes(b'fixture')
            provider = momso_server_install.GoogleProvider('fixture@example.invalid', 'fixture-project', '0123456789ab', root / 'key')
            with patch.object(provider, 'ssh') as ssh:
                provider.install(artifact, digest(b'fixture'))
            wrapper = ast.parse(ssh.call_args_list[-1].args[0])
            encoded = next(node.value.value for node in ast.walk(wrapper) if isinstance(node, ast.Assign) and isinstance(node.targets[0], ast.Name) and node.targets[0].id == 'p')
            inner = ast.parse(base64.b64decode(encoded))
            unit = next(node.value.value for node in ast.walk(inner) if isinstance(node, ast.Assign) and isinstance(node.targets[0], ast.Name) and node.targets[0].id == 'unit' and isinstance(node.value, ast.Constant))
            self.assertEqual(digest(unit.encode()), '40561a88e59e54a774e18104379607e2526a0c69fb3238deb7501f80ff4f7f4d')


if __name__ == '__main__':
    unittest.main()
