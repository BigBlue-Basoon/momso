import unittest
import io
import tarfile
import ast
import os
import subprocess
import sys
import tempfile
from pathlib import Path

from momso_anywhere_restore_files import RESTORE_FILES
import momso_anywhere_vm_services
from momso_anywhere_restore_bundle import restore_files


class RestoreFileTests(unittest.TestCase):
    def test_local_worker_imports_are_in_shared_manifest(self):
        directory = Path(__file__).resolve().parent
        delivered = set(RESTORE_FILES)
        for name in RESTORE_FILES:
            tree = ast.parse((directory / name).read_text(encoding='utf-8'))
            for node in ast.walk(tree):
                modules = [alias.name for alias in node.names] if isinstance(node, ast.Import) else [node.module] if isinstance(node, ast.ImportFrom) and node.module else []
                for module in modules:
                    first = module.split('.')[0]
                    if first.startswith('momso_anywhere_'):
                        self.assertIn(first + '.py', delivered, name)

    def test_signed_bundle_requires_exact_regular_restore_files(self):
        def bundle(names, *, link=False):
            target = io.BytesIO()
            with tarfile.open(fileobj=target, mode='w:gz') as archive:
                for name in names:
                    info = tarfile.TarInfo(name)
                    if link:
                        info.type = tarfile.SYMTYPE
                        info.linkname = '../outside'
                        archive.addfile(info)
                    else:
                        info.size = 1
                        archive.addfile(info, io.BytesIO(b'x'))
            return target.getvalue()
        self.assertEqual(set(restore_files(bundle(RESTORE_FILES))), set(RESTORE_FILES))
        for invalid in (bundle(RESTORE_FILES[:-1]), bundle((*RESTORE_FILES, RESTORE_FILES[0])), bundle(RESTORE_FILES, link=True)):
            with self.assertRaisesRegex(ValueError, 'restore_bundle_invalid'):
                restore_files(invalid)

    def test_pc_installs_the_complete_shared_set(self):
        self.assertTrue(set(RESTORE_FILES).issubset(momso_anywhere_vm_services._paths()))

    @unittest.skipUnless(os.name == 'posix', 'VM workers use Linux file locks')
    def test_pc_payload_can_import_restore_worker_in_isolation(self):
        with tempfile.TemporaryDirectory() as directory:
            for name, value in momso_anywhere_vm_services._paths().items():
                (Path(directory) / name).write_bytes(value)
            code = "import sys; sys.path.insert(0,sys.argv[1]); import momso_anywhere_prestart_restore, momso_anywhere_backup_agent"
            subprocess.run([sys.executable, '-I', '-c', code, directory], check=True)

    def test_updater_reads_signed_manifest_and_handles_legacy_bundle(self):
        path = Path(__file__).resolve().parent.parent / 'apps/alpha/desktop/src-tauri/momso-server-update/managed_helpers.py'
        source = path.read_text(encoding='utf-8')
        self.assertIn("release / 'momso-updater/momso_anywhere_restore_files.json'", source)
        self.assertIn('restore_files = LEGACY_HELPERS', source)

    def test_browser_bootstrap_uses_shared_manifest(self):
        source = Path(__file__).with_name('momso_anywhere_vm_startup.mjs').read_text(encoding='utf-8')
        self.assertIn("import RESTORE_FILES from './momso_anywhere_restore_files.json'", source)
        self.assertIn('backup_files=tuple(json.loads(', source)


if __name__ == '__main__':
    unittest.main()
