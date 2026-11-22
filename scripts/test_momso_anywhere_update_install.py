import ast
import base64
import hashlib
import io
import json
import os
from pathlib import Path
import sys
import tempfile
from types import SimpleNamespace
import unittest
from unittest.mock import Mock, patch

import momso_anywhere_update_install as install
from momso_server_install import InstallError


class InstallTests(unittest.TestCase):
    @unittest.skipIf(os.name == 'nt', 'VM payload requires Linux filesystem operations')
    def test_worker_upgrade_restores_old_files_when_install_fails(self):
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            opt = root / 'momso'; opt.mkdir()
            updater = root / 'momso-updater'; updater.mkdir()
            etc = root / 'etc'; etc.mkdir()
            for folder in (opt, updater, etc): folder.chmod(0o700)
            old = b'legacy worker'; (updater / 'worker.py').write_bytes(old)
            provider = Mock(project='fixture-project', install_id='0123456789ab')
            public = root / 'public.pem'; public.write_bytes(b'fixture key')
            with patch.object(install, 'LEGACY_WORKER', {'worker.py': [hashlib.sha256(old).hexdigest()]}):
                install.install(provider, 'development', public, 'https://fixture.invalid/release.json')
            wrapper, data = provider.ssh.call_args.args
            node = ast.parse(wrapper)
            encoded = next(n.args[0].value for n in ast.walk(node) if isinstance(n, ast.Call) and isinstance(n.func, ast.Attribute) and n.func.attr == 'b64decode')
            code = base64.b64decode(encoded).decode().replace('/opt/momso-updater', str(updater)).replace('/opt/momso', str(opt)).replace('/etc/', str(etc) + '/').replace("pathlib.Path('/etc')", 'pathlib.Path(' + repr(str(etc)) + ')')
            worker = SimpleNamespace(make_poller=lambda _config: SimpleNamespace(updater=object()))
            failure = SimpleNamespace(install=Mock(side_effect=RuntimeError('fixture failure')))
            ensure = SimpleNamespace(ensure=lambda: None)
            stdin = io.TextIOWrapper(io.BytesIO(data))
            with patch('sys.stdin', stdin), patch.dict(sys.modules, {'worker': worker, 'install_worker': failure, 'ensure_gcloud': ensure}):
                with self.assertRaisesRegex(RuntimeError, 'fixture failure'):
                    exec(compile(code, '<vm fixture>', 'exec'), {})
            self.assertEqual((updater / 'worker.py').read_bytes(), old)
            self.assertFalse((opt / 'momso_mobile_proxy.py').exists())
            self.assertFalse((etc / 'momso-update-public.pem').exists())

    def test_gateway_permission_is_scoped_to_owned_service_and_runtime(self):
        provider = Mock(project="fixture-project", install_id="0123456789ab", marker="fixture-owner")
        provider.run.side_effect = lambda *args, **kwargs: [] if kwargs.get("parse") else b""
        install.grant_gateway_update(provider)
        calls = [call.args for call in provider.run.call_args_list]
        binding = next(args for args in calls if args[:3] == ("run", "services", "add-iam-policy-binding"))
        self.assertEqual(binding[3], "momso-any-0123456789ab")
        self.assertIn("--role=projects/fixture-project/roles/momsoUpdate0123456789ab", binding)
        create = next(args for args in calls if args[:3] == ("iam", "roles", "create"))
        self.assertFalse(any("run.services.delete" in arg for arg in create))
        identity = next(args for args in calls if args[:3] == ("iam", "service-accounts", "add-iam-policy-binding"))
        self.assertIn("ma-0123456789ab" + "@" + "fixture-project.iam.gserviceaccount.com", identity)

    def test_payload_contains_channel_public_key_and_independent_worker(self):
        provider = Mock(project="fixture-project", install_id="0123456789ab")
        with tempfile.TemporaryDirectory() as directory:
            public = Path(directory) / "public.pem"
            public.write_text("fixture public key")
            install.install(provider, "development", public, "https://fixture.invalid/release.json")
        wrapper, data = provider.ssh.call_args.args
        node = ast.parse(wrapper)
        encoded = next(n.args[0].value for n in ast.walk(node) if isinstance(n, ast.Call) and isinstance(n.func, ast.Attribute) and n.func.attr == 'b64decode')
        code = base64.b64decode(encoded).decode()
        ast.parse(code)
        self.assertIn("install_group(folder,'update-worker'", code)
        payload = json.loads(data)
        self.assertIn("sudo", wrapper)
        self.assertEqual(payload["config"]["channel"], "development")
        self.assertEqual(payload["signature"], "https://fixture.invalid/release.json.sig")
        self.assertIn("worker.py", payload["files"])
        self.assertIn("install_worker.py", payload["files"])

    def test_invalid_manifest_url_fails_before_contacting_vm(self):
        provider = Mock()
        with self.assertRaises(InstallError):
            install.install(provider, "development", "unused", "http://localhost/release.json")
        provider.ssh.assert_not_called()
