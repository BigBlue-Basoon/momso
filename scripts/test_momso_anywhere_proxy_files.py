import ast
import base64
import hashlib
import json
import os
from pathlib import Path
import re
import shutil
import subprocess
import sys
import tarfile
import tempfile
import unittest

from momso_anywhere_proxy_files import PROXY_FILES


class ProxyFilesTests(unittest.TestCase):
    @unittest.skipIf(os.name == 'nt', 'updater imports Linux fcntl')
    def test_versioned_updater_imports_without_legacy_parent(self):
        scripts = Path(__file__).resolve().parent
        updater = scripts.parent / 'apps/alpha/desktop/src-tauri/momso-server-update'
        tree = ast.parse((updater / 'managed_helpers.py').read_text(encoding='utf-8'))
        workers = next(ast.literal_eval(item.value) for item in tree.body
                       if isinstance(item, ast.Assign)
                       and any(isinstance(target, ast.Name) and target.id == 'WORKER' for target in item.targets))
        with tempfile.TemporaryDirectory(prefix='momso-s173-worker-') as directory:
            target = Path(directory)
            for name in (*workers, 'momso_anywhere_restore_files.py', 'momso_anywhere_restore_files.json'):
                source = updater / name
                if not source.is_file():
                    source = scripts / name
                shutil.copy2(source, target / name)
            code = 'import sys; sys.path.insert(0,sys.argv[1]); import worker, update, github_releases'
            subprocess.run([sys.executable, '-I', '-c', code, str(target)], check=True, capture_output=True)

    def test_versioned_updater_includes_its_local_imports(self):
        updater = Path(__file__).resolve().parent.parent / 'apps/alpha/desktop/src-tauri/momso-server-update'
        tree = ast.parse((updater / 'managed_helpers.py').read_text(encoding='utf-8'))
        workers = next(ast.literal_eval(item.value) for item in tree.body
                       if isinstance(item, ast.Assign)
                       and any(isinstance(target, ast.Name) and target.id == 'WORKER' for target in item.targets))
        startup = (Path(__file__).resolve().parent / 'momso_anywhere_vm_startup.mjs').read_text(encoding='utf-8')
        match = re.search(r'^update_files=(\(.*\))$', startup, re.MULTILINE)
        self.assertIsNotNone(match, 'startup updater list is missing')
        startup_files = set(ast.literal_eval(match.group(1)))
        self.assertEqual(startup_files, set(workers) | {
            'momso_anywhere_restore_files.py', 'momso_anywhere_restore_files.json',
        })
        local = {path.stem for path in updater.glob('*.py') if not path.name.startswith('test_')}
        included = {Path(name).stem for name in workers}
        for name in workers:
            source = updater / name
            if not source.is_file():
                continue
            for node in ast.walk(ast.parse(source.read_text(encoding='utf-8'))):
                imports = ([node.module] if isinstance(node, ast.ImportFrom) else
                           [alias.name for alias in node.names] if isinstance(node, ast.Import) else [])
                for imported in imports:
                    module = (imported or '').split('.')[0]
                    if module in local:
                        self.assertIn(module, included, f'{name} imports missing {module}.py')

    def test_proxy_local_import_closure_is_in_manifest(self):
        scripts = Path(__file__).resolve().parent
        updater = scripts.parent / 'apps/alpha/desktop/src-tauri/momso-server-update'
        roots = [scripts, updater]
        known = {path.stem: path for root in roots for path in root.glob('*.py')}
        pending, visited = ['momso_anywhere_vm_proxy'], set()
        while pending:
            name = pending.pop()
            if name in visited:
                continue
            visited.add(name)
            self.assertIn(name + '.py', PROXY_FILES)
            tree = ast.parse(known[name].read_text(encoding='utf-8'))
            for node in ast.walk(tree):
                imports = ([node.module] if isinstance(node, ast.ImportFrom) else
                           [alias.name for alias in node.names] if isinstance(node, ast.Import) else [])
                for imported in imports:
                    module = (imported or '').split('.')[0]
                    if module in known:
                        pending.append(module)

    @unittest.skipIf(os.name == 'nt', 'proxy imports Linux fcntl')
    def test_three_delivery_paths_import_real_proxy(self):
        import momso_anywhere_install as installer
        import momso_anywhere_update_install as updater
        import prepare_momso_anywhere_bundle as bundle

        class FakeProvider:
            install_id = 'a' * 12
            project = 'fixture-project'
            def __init__(self): self.payload = None
            def ssh(self, _code, data=None):
                self.payload = json.loads(data)
                if 'proxyFiles' in self.payload: return b'worker_install_ok'
                return json.dumps({'fingerprint': 'a' * 64, 'token': 'b' * 40})

        with tempfile.TemporaryDirectory() as temp:
            root = Path(temp)
            provider = FakeProvider()
            installer.install_private_proxy(provider)
            first = {name: base64.b64decode(value) for name, value in provider.payload.items()}
            key = root / 'public.pem'; key.write_bytes(b'fixture')
            updater.install(provider, 'development', key, 'https://fixture.invalid/release.json')
            second = {name: base64.b64decode(value) for name, value in provider.payload['proxyFiles'].items()}
            server = root / 'server'; server.write_bytes(b'fixture')
            web = root / 'web'; web.mkdir(); (web / 'index.html').write_text('fixture')
            archive = root / 'bundle.tar.gz'
            bundle.prepare(server, hashlib.sha256(server.read_bytes()).hexdigest(), web, archive)
            with tarfile.open(archive, 'r:gz') as tar:
                third = {name: tar.extractfile(name).read() for name in PROXY_FILES}
            self.assertEqual(first, second)
            self.assertEqual(second, third)
            for index, payload in enumerate((first, second, third)):
                directory = root / str(index); directory.mkdir()
                for name, value in payload.items(): (directory / name).write_bytes(value)
                subprocess.run([sys.executable, '-c', 'import momso_anywhere_vm_proxy'], cwd=directory, check=True, env={'PATH': os.environ.get('PATH', '')}, capture_output=True)


if __name__ == '__main__':
    unittest.main()
