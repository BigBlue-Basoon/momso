import ast
import base64
import hashlib
import io
import json
import os
from pathlib import Path
import sys
import subprocess
import tempfile
from types import SimpleNamespace
import unittest
from unittest.mock import Mock, patch

import momso_anywhere_update_install as install
from momso_server_install import InstallError


class InstallTests(unittest.TestCase):
    def test_repair_rejects_different_account_before_contacting_vm(self):
        import momso_anywhere_repair_updater as repair
        with tempfile.TemporaryDirectory() as directory:
            state = Path(directory) / 'state.json'
            state.write_text(json.dumps({'version': 1, 'id': '0123456789ab', 'account': 'other@example.invalid', 'project': 'fixture-project'}))
            argv = ['repair', '--account', 'owner@example.invalid', '--project', 'fixture-project', '--state', str(state)]
            with patch('sys.argv', argv), patch.object(repair, 'refresh_worker') as refresh:
                with self.assertRaises(SystemExit): repair.main()
                refresh.assert_not_called()

    @unittest.skipIf(os.name == 'nt', 'VM payload requires Linux filesystem operations')
    def test_worker_refresh_preserves_recovery_journal_and_restarts_even_on_error(self):
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory); worker = root / 'worker'; worker.mkdir(mode=0o700)
            etc = root / 'etc'; etc.mkdir(mode=0o700); (etc / 'momso-private').mkdir(mode=0o700)
            (etc / 'momso-update.json').write_text('{}')
            unit = etc / 'systemd/system/momso-update.service'; unit.parent.mkdir(parents=True)
            unit.write_text(f'ExecStart=/usr/bin/python3 {worker}/worker.py --config {etc}/momso-update.json\n')
            recovery = root / 'update-state.json'; recovery.write_text('{"phase":"switching"}')
            provider = Mock(install_id='0123456789ab')
            install.refresh_worker(provider)
            wrapper, payload = provider.ssh.call_args.args
            node = ast.parse(wrapper)
            encoded = next(n.args[0].value for n in ast.walk(node) if isinstance(n, ast.Call) and isinstance(n.func, ast.Attribute) and n.func.attr == 'b64decode')
            code = base64.b64decode(encoded).decode().replace('/opt/momso-updater', str(worker)).replace('/etc/', str(etc) + '/')
            calls = []
            with patch('sys.stdin', io.TextIOWrapper(io.BytesIO(payload))), patch('subprocess.run', side_effect=lambda args, **kwargs: calls.append(args)):
                exec(compile(code, '<vm fixture>', 'exec'), {})
            self.assertEqual(calls, [['systemctl', 'stop', 'momso-update.service'], ['systemctl', 'start', 'momso-update.service']])
            self.assertEqual(json.loads(recovery.read_text())['phase'], 'switching')
            calls.clear()
            def failed_stop(args, **kwargs):
                calls.append(args)
                if args[1] == 'stop': raise RuntimeError('stop failed')
            with patch('sys.stdin', io.TextIOWrapper(io.BytesIO(payload))), patch('subprocess.run', side_effect=failed_stop):
                with self.assertRaisesRegex(RuntimeError, 'stop failed'): exec(compile(code, '<vm fixture>', 'exec'), {})
            self.assertEqual(calls[-1], ['systemctl', 'start', 'momso-update.service'])
            (worker / 'worker.py').write_bytes(b'unknown worker')
            calls.clear()
            with patch('sys.stdin', io.TextIOWrapper(io.BytesIO(payload))), patch('subprocess.run', side_effect=lambda args, **kwargs: calls.append(args)):
                with self.assertRaises(Exception): exec(compile(code, '<vm fixture>', 'exec'), {})
            self.assertEqual(calls[-1], ['systemctl', 'start', 'momso-update.service'])
            self.assertEqual((worker / 'worker.py').read_bytes(), b'unknown worker')

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
            provider.ssh.return_value = b'worker_install_ok'
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
            with patch('sys.stdin', stdin), patch('subprocess.run', return_value=subprocess.CompletedProcess([], 1)), patch.dict(sys.modules, {'worker': worker, 'install_worker': failure, 'ensure_gcloud': ensure}):
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
        provider.ssh.return_value = b'worker_install_ok'
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
        self.assertIn('gate_already_held=data[\'gateAlreadyHeld\']', code)
        self.assertIn('/__momso/proxy-health', code)
        payload = json.loads(data)
        self.assertIn("sudo", wrapper)
        self.assertEqual(payload["config"]["channel"], "development")
        self.assertEqual(payload["signature"], "https://fixture.invalid/release.json.sig")
        self.assertIn("worker.py", payload["files"])
        self.assertIn("install_worker.py", payload["files"])

    def test_remote_proxy_health_failure_has_machine_code(self):
        provider = Mock(project='fixture-project', install_id='0123456789ab')
        provider.ssh.return_value = b'proxy_not_running\n'
        with tempfile.TemporaryDirectory() as directory:
            public = Path(directory) / 'public.pem'; public.write_bytes(b'fixture')
            with self.assertRaises(InstallError) as error:
                install.install(provider, 'development', public, 'https://fixture.invalid/release.json', gate_already_held=True, health_url='https://fixture.a.run.app')
        self.assertEqual(error.exception.gateway_code, 'proxy_not_running')

    def test_invalid_manifest_url_fails_before_contacting_vm(self):
        provider = Mock()
        with self.assertRaises(InstallError):
            install.install(provider, "development", "unused", "http://localhost/release.json")
        provider.ssh.assert_not_called()

    def test_production_bootstrap_contains_three_keys_at_sequence_zero(self):
        import momso_release_keys as keys
        import momso_release_sign as signer
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            public_keys = signer.init_production(root / 'keys')
            encoded = [base64.b64encode(keys.public_der(path)).decode() for path in public_keys]
            bootstrap = root / 'root.json'
            value = {'version': 0, 'expires': '2099-01-01T00:00:00Z', 'threshold': 2, 'keys': encoded, 'releaseKey': encoded[0]}
            bootstrap.write_text(json.dumps(value), encoding='utf-8')
            self.assertEqual(json.loads(install.production_bootstrap(public_keys[0], bootstrap))['version'], 0)
            with self.assertRaises(InstallError): install.production_bootstrap(public_keys[1], bootstrap)
            with patch.object(keys, 'DEVELOPMENT_KEY', public_keys[1]):
                with self.assertRaises(InstallError): install.production_bootstrap(public_keys[0], bootstrap)
            value['version'] = 1
            bootstrap.write_text(json.dumps(value), encoding='utf-8')
            with self.assertRaises(InstallError): install.production_bootstrap(public_keys[0], bootstrap)

    def test_production_installer_sends_root_zero_to_vm_worker(self):
        import momso_release_keys as keys
        import momso_release_sign as signer
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            public = signer.init_production(root / 'keys')
            encoded = [base64.b64encode(keys.public_der(path)).decode() for path in public]
            raw = json.dumps({'version': 0, 'expires': '2099-01-01T00:00:00Z', 'threshold': 2, 'keys': encoded, 'releaseKey': encoded[0]}).encode()
            root_file = root / 'root.json'; root_file.write_bytes(raw)
            validate = install.production_bootstrap
            provider = Mock(project='fixture-project', install_id='0123456789ab')
            provider.ssh.return_value = b'worker_install_ok'
            with patch.object(install, 'production_bootstrap', side_effect=lambda key: validate(key, root_file)):
                install.install(provider, 'production', public[0], 'https://fixture.invalid/release.json')
            payload = json.loads(provider.ssh.call_args.args[1])
            self.assertEqual(base64.b64decode(payload['config']['trustBootstrap']), raw)
            self.assertEqual(payload['config']['trustState'], '/var/lib/momso-update-trust')
            self.assertIn('trust_rotation.py', payload['files'])
            import prepare_momso_anywhere_start_site as start_site
            dist = root / 'dist'; (dist / 'assets').mkdir(parents=True)
            (dist / 'assets/app.js').write_text('resumeAfterRedirect')
            (dist / 'index.html').write_text('<script type="module" src="/start/assets/app.js"></script>')
            site = root / 'site'
            with patch.object(keys, 'PRODUCTION_KEY', public[0]), patch.object(keys, 'DEVELOPMENT_KEY', root / 'missing-dev.pem'):
                start_site.prepare(dist, site, 'fixture.apps.googleusercontent.com', 'https://fixture.invalid/release.json', public[0], 'https://fixture.invalid', root_path=root_file)
            site_release = json.loads((site / 'config.json').read_text())['release']
            self.assertEqual(site_release['trustBootstrap'], payload['config']['trustBootstrap'])
            source = "import {vmStartupScript} from './scripts/momso_anywhere_vm_startup.mjs'; const update=JSON.parse(process.argv[1]); const s=vmStartupScript({project:'fixture-project',secret:'momso-transport-aaaaaaaaaaaa',bundleUrl:'https://fixture.invalid/bundle',bundleSha256:'a'.repeat(64),update}); if(!s.includes(update.trustBootstrap))process.exit(1); console.log(update.trustBootstrap)"
            browser_config = site_release
            result = subprocess.run(['node', '--input-type=module', '-e', source, json.dumps(browser_config)], check=True, capture_output=True, text=True)
            self.assertEqual(result.stdout.strip(), payload['config']['trustBootstrap'])
