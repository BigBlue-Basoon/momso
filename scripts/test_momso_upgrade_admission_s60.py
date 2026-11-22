import ast
import base64
from io import BytesIO
import json
import os
from pathlib import Path
import subprocess
import sys
import tempfile
from types import SimpleNamespace
import unittest
from unittest.mock import patch
from urllib.error import HTTPError

import momso_server_install as installer


class UpgradeAdmissionTests(unittest.TestCase):
    def exercise(self, mode, busy_sequence=(), timeout=False):
        with tempfile.TemporaryDirectory() as tmp:
            root = Path(tmp) / 'opt'; root.mkdir()
            web = root / 'web'; web.mkdir(); (web / '.momso-owned').write_text('momso')
            (root / 'momso-server').write_text('old')
            data = Path(tmp) / 'data'; data.mkdir(); (data / 'db').write_text('fixture')
            env = Path(tmp) / 'server.env'; env.write_text('MOMSO_SERVER_TOKEN=fixture\nMOMSO_SERVER_DATA_DIR=' + str(data) + '\n')
            with patch.object(installer.shutil, 'which', return_value='gcloud'):
                provider = installer.GoogleProvider('fixture@example.test', 'fixture-project', '0123456789ab', Path(tmp) / 'key')
            with patch.object(provider, 'ssh', return_value=b'momso_checkpoint_ok') as ssh: provider.upgrade_checkpoint('begin')
            tree = ast.parse(ssh.call_args.args[0])
            encoded = next(n.args[0].value for n in ast.walk(tree) if isinstance(n, ast.Call) and isinstance(n.func, ast.Attribute) and n.func.attr == 'b64decode')
            payload = base64.b64decode(encoded).decode().replace("root='/opt/momso'", 'root=' + repr(str(root))).replace("env='/etc/momso-server.env'", 'env=' + repr(str(env)))
            busy = list(busy_sequence); calls = []; running = []
            def response(req, timeout):
                calls.append((req.full_url.rsplit('/', 1)[-1], req.method))
                if req.full_url.endswith('/activity'):
                    if mode == 'absent': raise HTTPError(req.full_url, 404, 'fixture', {}, None)
                    is_busy = busy.pop(0) if busy else timeout_busy
                    value = {'busy': is_busy, 'activeTurns': int(is_busy)}
                    if mode in ('modern', 'legacy_404'): value.update(maintenance=False, quiescent=not is_busy, maintenanceReady=False, maintenanceReason='backup_not_integrated')
                    return BytesIO(json.dumps(value).encode())
                if req.full_url.endswith('/maintenance'):
                    if mode == 'legacy_404': raise HTTPError(req.full_url, 404, 'fixture', {}, None)
                    return BytesIO(b'{"maintenance":true}')
                return BytesIO(b'{"maintenance":false}')
            timeout_busy = bool(timeout)
            fcntl = SimpleNamespace(LOCK_EX=2, LOCK_NB=4, flock=lambda *_: None)
            def system(*args, **kwargs): running.append(args[0]); return SimpleNamespace(returncode=0)
            context = {}
            clock = [0, 121] if timeout else [0] * 10
            with patch.dict(sys.modules, {'fcntl': fcntl}), patch.object(os, 'O_NOFOLLOW', 0, create=True), patch.object(os, 'chown', lambda *_, **__: None, create=True), patch.object(os, 'fchown', lambda *_: None, create=True), patch('urllib.request.urlopen', side_effect=response), patch.object(subprocess, 'run', side_effect=system), patch('time.monotonic', side_effect=clock), patch('time.sleep'):
                try:
                    if timeout:
                        with self.assertRaisesRegex(RuntimeError, 'server_busy'): exec(compile(payload, 'fixture', 'exec'), context)
                    else:
                        exec(compile(payload, 'fixture', 'exec'), context)
                finally:
                    if 'lock' in context: os.close(context['lock'])
            return calls, running, (root / '.momso-upgrade-backup').exists()

    def test_busy_result_is_explained_to_user(self):
        with tempfile.TemporaryDirectory() as tmp, patch.object(installer.shutil, 'which', return_value='gcloud'):
            provider = installer.GoogleProvider('fixture@example.test', 'fixture-project', '0123456789ab', Path(tmp) / 'key')
            with patch.object(provider, 'ssh', return_value=b'momso_server_busy\n'):
                with self.assertRaisesRegex(installer.InstallError, '지금 쓰는 중'):
                    provider.upgrade_checkpoint('begin')

    def test_backup_not_integrated_does_not_block_quiescent_server(self):
        calls, commands, saved = self.exercise('modern')
        self.assertTrue(saved); self.assertIn(('maintenance', 'POST'), calls)
        self.assertTrue(any('stop' in cmd for cmd in commands))

    def test_legacy_server_waits_for_running_turn(self):
        calls, commands, saved = self.exercise('legacy', [True, True, False])
        self.assertTrue(saved); self.assertGreaterEqual(sum(x[0] == 'activity' for x in calls), 3)
        self.assertTrue(any('stop' in cmd for cmd in commands)); self.assertFalse(any('start' in cmd for cmd in commands))

    def test_legacy_missing_maintenance_endpoint(self):
        calls, commands, saved = self.exercise('legacy_404')
        self.assertTrue(saved); self.assertIn(('maintenance', 'POST'), calls)

    def test_no_activity_stops_before_snapshot(self):
        calls, commands, saved = self.exercise('absent')
        self.assertTrue(saved); self.assertTrue(any('stop' in cmd for cmd in commands))
        self.assertFalse(any(x[0] == 'maintenance' for x in calls))

    def test_busy_timeout_never_stops_server(self):
        calls, commands, saved = self.exercise('legacy', [True, True], timeout=True)
        self.assertFalse(saved); self.assertFalse(any('stop' in cmd for cmd in commands))


if __name__ == '__main__': unittest.main()
