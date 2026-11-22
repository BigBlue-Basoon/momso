import ast
import base64
import hashlib
import json
import os
import subprocess
import sys
import tempfile
import unittest
from io import BytesIO
from pathlib import Path
from types import SimpleNamespace
from unittest.mock import patch
from urllib.error import HTTPError

import momso_anywhere_install
import momso_anywhere_public_assets
import momso_anywhere_vm_services as services
import momso_server_install as installer
from momso_anywhere_backup_provision import GoogleBackupProvisioner


def inner_code(wrapper):
    tree = ast.parse(wrapper)
    encoded = next(node.args[0].value for node in ast.walk(tree)
                   if isinstance(node, ast.Call) and isinstance(node.func, ast.Attribute)
                   and node.func.attr == 'b64decode')
    return base64.b64decode(encoded).decode()


class UpgradeGateTests(unittest.TestCase):
    def exercise(self, *, fail=False, upgrade=True, mode='gated', fail_attach=False, fail_restore_health=False):
        with tempfile.TemporaryDirectory() as tmp:
            base = Path(tmp); root = base / 'opt'; root.mkdir()
            web = root / 'web'; web.mkdir(); (web / '.momso-owned').write_text('owned')
            (web / 'index.html').write_text('old')
            (root / 'momso-server').write_text('old')
            data = base / 'data'; data.mkdir(); (data / 'record').write_text('old')
            env = base / 'server.env'; env.write_text('MOMSO_SERVER_TOKEN=fixture\nMOMSO_SERVER_DATA_DIR=' + str(data) + '\n')
            private = base / 'private'; lifecycle = base / 'lifecycle'
            artifact = base / 'new-server'; artifact.write_bytes(b'new')
            web_bundle = base / 'web.tar.gz'; web_bundle.write_bytes(b'web')
            fake = {'maintenance': False, 'events': [], 'server': 'old'}
            def response(req, timeout):
                path = req.full_url.split(':8765')[-1]
                fake['events'].append((path, req.method))
                if path == '/maintenance' and mode == 'legacy' and fake['server'] == 'old':
                    raise HTTPError(req.full_url, 404, 'fixture', {}, None)
                if path == '/maintenance': fake['maintenance'] = True
                if path == '/maintenance/release': fake['maintenance'] = False
                return BytesIO(json.dumps({'maintenance': fake['maintenance'], 'quiescent': fake['maintenance'], 'busy': False}).encode())
            snapshot_stub = b'''import os\nclass DataSnapshots:\n def __init__(self,data,root): self.root=root\n def snapshot(self):\n  path=os.path.join(self.root,'fixture');os.makedirs(path);return path\n def restore(self,path): pass\n'''
            original_decode = base64.b64decode
            def checkpoint_payload(action):
                with patch.object(installer.shutil, 'which', return_value='gcloud'):
                    p = installer.GoogleProvider('fixture@example.test', 'fixture-project', 'abcdef123456', base / 'key')
                with patch.object(p, 'ssh', return_value=b'momso_checkpoint_ok') as call:
                    p.upgrade_checkpoint(action)
                code = inner_code(call.call_args.args[0])
                return code.replace("root='/opt/momso'", 'root=' + repr(str(root))).replace("env='/etc/momso-server.env'", 'env=' + repr(str(env)))
            def run_code(code):
                code = code.replace('/var/lib/momso/lifecycle', str(lifecycle).replace('\\', '/'))
                code = code.replace('/etc/momso-private', str(private).replace('\\', '/'))
                code = code.replace('/etc/momso-server.env', str(env).replace('\\', '/'))
                code = code.replace('/opt/momso/.momso-upgrade-backup', str(root / '.momso-upgrade-backup').replace('\\', '/'))
                fcntl = SimpleNamespace(LOCK_EX=2, LOCK_NB=4, flock=lambda *_: None)
                pwd = SimpleNamespace(getpwnam=lambda _: SimpleNamespace(pw_uid=0, pw_gid=0))
                def decode(value, *args, **kwargs):
                    result = original_decode(value, *args, **kwargs)
                    return snapshot_stub if b'class DataSnapshots' in result else result
                with patch.dict(sys.modules, {'fcntl': fcntl, 'pwd': pwd}), patch.object(os, 'O_NOFOLLOW', 0, create=True), patch.object(os, 'chown', lambda *a: None, create=True), patch.object(os, 'fchown', lambda *a: None, create=True), patch('urllib.request.urlopen', side_effect=response), patch.object(subprocess, 'run', return_value=SimpleNamespace(returncode=0)), patch.object(base64, 'b64decode', side_effect=decode):
                    scope = {}
                    try:
                        exec(compile(code, '<fixture>', 'exec'), scope)
                    except SystemExit as error:
                        if error.code not in (None, 0): raise
                    finally:
                        for name in ('fd', 'lock'):
                            if isinstance(scope.get(name), int):
                                try: os.close(scope[name])
                                except OSError: pass
            class Provider:
                name = 'momso-abcdef123456'; network = name + '-net'; install_id = 'abcdef123456'; project = 'fixture-project'
                def run(self, *args, **kwargs):
                    if args[:3] == ('compute', 'instances', 'describe'):
                        return {'labels': {'momso-install-id': self.install_id}, 'status': 'RUNNING', 'networkInterfaces': [{'network': 'net/' + self.network}], 'serviceAccounts': [] if fail_attach else [{'email': 'mv-' + self.install_id + '@' + self.project + '.iam.gserviceaccount.com', 'scopes': [services.SCOPE]}]}
                    if fail_attach and args[:3] == ('compute', 'instances', 'stop'):
                        raise installer.InstallError('fixture attach failure')
                    if args[:3] == ('run', 'services', 'describe'): return {'status': {'url': 'https://fixture.invalid'}}
                    return [] if kwargs.get('parse') else b''
                def ssh(self, wrapper, data=None): run_code(inner_code(wrapper)); return b''
                def upgrade_checkpoint(self, action):
                    run_code(checkpoint_payload(action))
                    if action == 'restore': fake['restored'] = True
                def upgrade_server(self, *_):
                    self.assert_gated(); fake['server'] = 'new'; (root / 'momso-server').write_text('new')
                def install_web(self, *_): (web / 'index.html').write_text('new')
                def health(self):
                    if fail_restore_health and fake.get('restored'): raise installer.InstallError('fixture restored health failure')
                def ensure_private_proxy(self): fake['backup_preserved'] = (root / '.momso-upgrade-backup').exists()
                def assert_gated(self):
                    if mode == 'gated' and not fake['maintenance']: raise AssertionError('upgrade must own the gate')
            provider = Provider()
            with patch.object(services, 'identity', return_value='mv-abcdef123456' + '@' + 'fixture-project.iam.gserviceaccount.com'), patch.object(services, 'secret', return_value=('fixture', False)), patch.object(GoogleBackupProvisioner, 'provision', return_value={'bucket': 'fixture', 'key': 'fixture'}), patch.object(services, 'install_scripts', side_effect=installer.InstallError('fixture failure') if fail and not fail_attach else None), patch.object(momso_anywhere_public_assets, 'from_web_archive', return_value={}), patch.object(momso_anywhere_install.GoogleAnywhere, '_objects', return_value=({'owned': True}, {'owned': True}, {'owned': True})), patch.object(momso_anywhere_install.GoogleAnywhere, 'upgrade_gateway'):
                if upgrade:
                    state = {'anywhere': {'client_id': 'client', 'owner_sub': 'owner', 'login_origin': 'https://login.invalid', 'url': 'https://fixture.invalid'}}
                    args = (provider, state, artifact, hashlib.sha256(artifact.read_bytes()).hexdigest(), web_bundle, hashlib.sha256(web_bundle.read_bytes()).hexdigest(), 'client', 'owner', 'https://login.invalid', 'registry.example/fixture@sha256:' + 'a' * 64)
                    if fail:
                        with self.assertRaises(installer.InstallError): installer.upgrade_anywhere(*args)
                    else: installer.upgrade_anywhere(*args)
                else:
                    services.install(provider)
            return fake, (root / 'momso-server').read_text(), (web / 'index.html').read_text(), (private / 's56-identity-gate').exists()

    def test_upgrade_borrows_gate_and_finish_releases_once(self):
        fake, server, web, marker = self.exercise()
        self.assertEqual((server, web, marker, fake['maintenance']), ('new', 'new', False, False))
        self.assertEqual(fake['events'].count(('/maintenance', 'POST')), 1)
        self.assertEqual(fake['events'].count(('/maintenance/release', 'POST')), 1)

    def test_failed_upgrade_rolls_back_and_releases_once(self):
        fake, server, web, marker = self.exercise(fail=True)
        self.assertEqual((server, web, marker, fake['maintenance']), ('old', 'old', False, False))
        self.assertEqual(fake['events'].count(('/maintenance', 'POST')), 1)
        self.assertEqual(fake['events'].count(('/maintenance/release', 'POST')), 1)

    def test_first_install_acquires_and_releases_its_own_gate(self):
        fake, _, _, marker = self.exercise(upgrade=False)
        self.assertFalse(fake['maintenance']); self.assertFalse(marker)
        self.assertEqual(fake['events'].count(('/maintenance', 'POST')), 1)
        self.assertEqual(fake['events'].count(('/maintenance/release', 'POST')), 1)

    def test_upgrade_from_legacy_server_acquires_its_own_gate(self):
        fake, server, web, marker = self.exercise(mode='legacy')
        self.assertEqual((server, web, marker, fake['maintenance']), ('new', 'new', False, False))
        self.assertEqual(fake['events'].count(('/maintenance', 'POST')), 2)
        self.assertEqual(fake['events'].count(('/maintenance/release', 'POST')), 1)

    def test_failed_attach_releases_gate_owned_by_upgrade(self):
        fake, server, web, marker = self.exercise(fail=True, fail_attach=True)
        self.assertEqual((server, web, marker, fake['maintenance']), ('old', 'old', False, False))
        self.assertEqual(fake['events'].count(('/maintenance/release', 'POST')), 1)

    def test_failed_attach_releases_legacy_installers_own_gate(self):
        fake, server, web, marker = self.exercise(fail=True, fail_attach=True, mode='legacy')
        self.assertEqual((server, web, marker, fake['maintenance']), ('old', 'old', False, False))
        self.assertEqual(fake['events'].count(('/maintenance/release', 'POST')), 1)

    def test_restore_health_failure_releases_gate_but_keeps_recovery_copy(self):
        fake, server, web, marker = self.exercise(fail=True, fail_restore_health=True)
        self.assertEqual((server, web, marker, fake['maintenance']), ('old', 'old', False, False))
        self.assertTrue(fake['backup_preserved'])
        self.assertEqual(fake['events'].count(('/maintenance/release', 'POST')), 1)


if __name__ == '__main__': unittest.main()
