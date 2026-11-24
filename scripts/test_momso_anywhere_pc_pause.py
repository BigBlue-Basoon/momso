import json
import subprocess
import sys
import tempfile
import time
import unittest
from pathlib import Path
from unittest.mock import patch

from momso_anywhere_pc_pause import PcPause, REMOTE_RUNNER, guarded_destroy
from momso_server_install import InstallError


class Provider:
    install_id = 'abcdef123456'
    name = 'momso-abcdef123456'
    def __init__(self): self.running = True; self.deleted = False; self.calls = []
    def run(self, *args, **kwargs):
        self.calls.append(args)
        if args[:3] == ('compute', 'instances', 'describe'):
            return {'status': 'RUNNING' if self.running else 'TERMINATED', 'labels': {'momso-install-id': self.install_id}}
        if args[:3] == ('compute', 'instances', 'list'):
            return [] if self.deleted else [{'name': self.name, 'status': 'RUNNING', 'labels': {'momso-install-id': self.install_id}}]
        raise AssertionError(args)
    def ssh(self, code):
        self.calls.append(('ssh', code))
        return b'{}'


class Tests(unittest.TestCase):
    def test_remote_process_holds_gate_until_abort_after_commit(self):
        fake = '''class ServerAdmission:
 def __init__(self,*a,**k): self.held=False; self.closed_at=100
 def _request(self,path):
  return {'maintenance':self.held,'quiescent':self.held,'busy':False,'maintenanceReady':True,'githubBackup':{'state':'saved','lastSuccessAt':101},'objectBackup':{'state':'not_configured'}}
 def acquire(self): self.held=True; return True
 def release(self): self.held=False
'''
        with tempfile.TemporaryDirectory() as tmp:
            root = Path(tmp)
            (root / 'server_admission.py').write_text(fake, encoding='utf8')
            source = Path(__file__).with_name('momso_anywhere_pause_guard.py')
            (root / 'momso_anywhere_pause_guard.py').write_bytes(source.read_bytes())
            process = subprocess.Popen([sys.executable, '-c', REMOTE_RUNNER, str(root)], stdout=subprocess.DEVNULL, stderr=subprocess.DEVNULL)
            self.addCleanup(lambda: process.kill() if process.poll() is None else None)
            def wait(expected):
                deadline = time.monotonic() + 10
                while time.monotonic() < deadline:
                    path = root / 'status.json'
                    if path.exists() and json.loads(path.read_text(encoding='utf8')).get('state') == expected: return
                    time.sleep(.05)
                self.fail('remote guard did not reach ' + expected)
            try:
                wait('ready')
                (root / 'control').write_text('commit', encoding='ascii')
                wait('committed')
                self.assertIsNone(process.poll())
                (root / 'control').write_text('abort', encoding='ascii')
                wait('released')
                self.assertEqual(process.wait(timeout=3), 0)
            finally:
                if process.poll() is None: process.kill(); process.wait()

    def test_remote_transport_stages_code_via_existing_scp_ssh(self):
        p = Provider(); guard = PcPause(p, sleep=lambda _: None)
        guard._root_python('x' * 20000)
        self.assertEqual(p.calls[-1][0], 'ssh')
        # provider.ssh transfers this payload as a file; it is not a gcloud argument.
        self.assertIn('base64.b64decode', p.calls[-1][1])

    def test_backup_unconnected_or_failed_does_not_delete(self):
        p = Provider()
        for reason, expected in [('backup_not_connected', 'backup_not_connected'), ('backup_failed', 'backup_freshness_unverified')]:
            guard = PcPause(p, sleep=lambda _: None, attempts=1)
            with patch.object(guard, '_root_python', side_effect=[b'', json.dumps({'state': 'released', 'reason': reason}).encode()]):
                with self.assertRaisesRegex(InstallError, expected): guard.start()

    def test_stopped_vm_never_starts_guard(self):
        p = Provider(); p.running = False
        with self.assertRaisesRegex(InstallError, 'vm_unreachable'):
            PcPause(p, sleep=lambda _: None).start()
        self.assertFalse(any(call[0] == 'ssh' for call in p.calls))

    def test_unconfirmed_release_is_not_reported_as_open(self):
        guard = PcPause(Provider(), sleep=lambda _: None)
        with patch.object(guard, '_control'), patch.object(guard, '_status', return_value={'state': 'failed', 'reason': 'maintenance_release_unconfirmed'}):
            with self.assertRaisesRegex(InstallError, 'maintenance_release_unconfirmed'):
                guard.abort()

    def test_gate_held_until_vm_is_confirmed_deleted(self):
        p = Provider(); events = []
        class Guard:
            def __init__(self, provider): self.provider = provider
            def start(self): events.append('backup_confirmed')
            def commit(self): events.append('gate_committed')
            def abort(self): events.append('gate_released')
        def delete(): events.append('delete'); p.deleted = True
        guarded_destroy(p, delete, guard_factory=Guard)
        self.assertEqual(events, ['backup_confirmed', 'gate_committed', 'delete'])
        self.assertEqual(p.calls[-1][:3], ('compute', 'instances', 'list'))

    def test_delete_failure_releases_gate_on_running_owned_vm(self):
        p = Provider(); events = []
        class Guard:
            def __init__(self, provider): pass
            def start(self): events.append('start')
            def commit(self): events.append('commit')
            def abort(self): events.append('abort')
        def fail(): raise InstallError('fixture')
        with self.assertRaisesRegex(InstallError, 'fixture'):
            guarded_destroy(p, fail, guard_factory=Guard)
        self.assertEqual(events, ['start', 'commit', 'abort'])

    def test_unknown_delete_result_never_releases_gate(self):
        p = Provider(); events = []
        class Guard:
            def __init__(self, provider): pass
            def start(self): pass
            def commit(self): pass
            def abort(self): events.append('abort')
        p.run = lambda *args, **kwargs: (_ for _ in ()).throw(InstallError('network'))
        with self.assertRaisesRegex(InstallError, 'maintenance_release_unconfirmed'):
            guarded_destroy(p, lambda: (_ for _ in ()).throw(InstallError('fixture')), guard_factory=Guard)
        self.assertEqual(events, [])


if __name__ == '__main__': unittest.main()
