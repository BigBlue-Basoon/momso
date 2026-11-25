import ast
from contextlib import redirect_stdout
import io
import json
import os
from pathlib import Path
import subprocess
import shutil
import sys
import tempfile
import time
import unittest
import uuid
from unittest.mock import patch

from momso_anywhere_pc_vm import GUARD, RECOVER, VmLease, ensure_vm_ready, existing_vm
from momso_server_install import InstallError


class Provider:
    install_id = 'a' * 12
    name = 'momso-' + install_id
    def __init__(self, states, *, fail_start=False, ssh_failures=0):
        self.states = list(states); self.calls = []; self.fail_start = fail_start; self.ssh_failures = ssh_failures
    def run(self, *args, **_kwargs):
        self.calls.append(args)
        if args[:3] == ('compute', 'instances', 'describe'):
            value = self.states.pop(0) if len(self.states) > 1 else self.states[0]
            if value is None: raise InstallError('unreadable')
            return {'status': value, 'labels': {'momso-install-id': self.install_id}}
        if args[:3] == ('compute', 'instances', 'start') and self.fail_start: raise InstallError('failure')
        return []
    def _ssh_inline(self, code, **_kwargs):
        self.calls.append(('ssh', code))
        if self.ssh_failures:
            self.ssh_failures -= 1
            raise subprocess.TimeoutExpired('ssh', 1)


class Tests(unittest.TestCase):
    def test_stopped_vm_starts_before_ssh(self):
        provider = Provider(['TERMINATED', 'STAGING', 'RUNNING'], ssh_failures=1)
        ensure_vm_ready(provider, sleep=lambda _: None)
        self.assertEqual(sum(call[:3] == ('compute', 'instances', 'start') for call in provider.calls), 1)
        self.assertLess(next(i for i, call in enumerate(provider.calls) if call[:3] == ('compute', 'instances', 'start')),
                        next(i for i, call in enumerate(provider.calls) if call[0] == 'ssh'))

    def test_running_vm_never_starts_again(self):
        provider = Provider(['RUNNING'])
        ensure_vm_ready(provider, sleep=lambda _: None)
        self.assertFalse(any(call[:3] == ('compute', 'instances', 'start') for call in provider.calls))

    def test_existing_vm_checks_the_full_owner_label(self):
        provider = Provider(['RUNNING'])
        provider.run = lambda *_args, **_kwargs: [{'name': provider.name, 'labels': {'momso-install-id': provider.install_id}}]
        self.assertTrue(existing_vm(provider))
        provider.run = lambda *_args, **_kwargs: [{'name': provider.name, 'labels': {'momso-install-id': 'b' * 12}}]
        with self.assertRaisesRegex(InstallError, 'vm_state_unknown'):
            existing_vm(provider)

    def test_start_state_and_ssh_errors_are_distinct(self):
        for provider, reason in ((Provider([None]), 'vm_state_unknown'), (Provider(['TERMINATED'], fail_start=True), 'vm_start_failed'), (Provider(['RUNNING'], ssh_failures=3), 'vm_unreachable')):
            with self.subTest(reason=reason), self.assertRaisesRegex(InstallError, reason):
                ensure_vm_ready(provider, sleep=lambda _: None, ssh_attempts=3)

    def test_guard_has_expiring_lease_and_lock_aware_recovery(self):
        ast.parse(GUARD, feature_version=(3, 11)); ast.parse(RECOVER, feature_version=(3, 11))
        self.assertIn('lifecycle.lock', GUARD)
        self.assertIn('pc-installer.lock', GUARD)
        self.assertIn("time.time()>=float(p.read_text())", GUARD)
        self.assertIn('pc-installer.lock', RECOVER)
        self.assertIn("['systemctl','start','momso-idle-stop.service']", RECOVER)

    def test_operation_failure_still_releases_lease(self):
        class FakeLease(VmLease):
            def __init__(self): super().__init__(Provider(['RUNNING'])); self.states = iter([{'state': 'ready'}, {'state': 'released'}]); self.commands = []
            def _root(self, code, data=None): self.commands.append(code); return b''
            def _status(self): return next(self.states)
        lease = FakeLease()
        with patch('momso_anywhere_pc_vm.ensure_vm_ready'):
            with self.assertRaisesRegex(RuntimeError, 'fixture'):
                with lease:
                    raise RuntimeError('fixture')
        self.assertFalse(lease.started)
        self.assertTrue(any("'stop'" in code for code in lease.commands))

    def test_failed_guard_is_stopped_before_reporting_error(self):
        class FakeLease(VmLease):
            def __init__(self): super().__init__(Provider(['RUNNING'])); self.states = iter([{'state': 'failed', 'reason': 'vm_busy'}, {'state': 'released'}]); self.commands = []
            def _root(self, code, data=None): self.commands.append(code); return b''
            def _status(self): return next(self.states)
        lease = FakeLease()
        with patch('momso_anywhere_pc_vm.ensure_vm_ready'), self.assertRaisesRegex(InstallError, 'vm_busy'):
            with lease: self.fail('must not enter')
        self.assertFalse(lease.started)
        self.assertTrue(any("'stop'" in code for code in lease.commands))

    def test_updater_repair_uses_the_vm_lease(self):
        import momso_anywhere_repair_updater as repair
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory); state = root / 'state.json'; (root / 'ssh').write_text('fixture')
            state.write_text(json.dumps({'version': 1, 'id': 'a' * 12, 'account': 'fixture@example.test', 'project': 'fixture-project'}))
            active = []
            class Lease:
                def __init__(self, _provider): pass
                def __enter__(self): active.append('entered')
                def __exit__(self, *_): active.append('released')
            with patch.object(sys, 'argv', ['repair', '--account', 'fixture@example.test', '--project', 'fixture-project', '--state', str(state)]), patch.object(repair, 'GoogleProvider', return_value=object()), patch.object(repair, 'key_directory', return_value=root), patch.object(repair, 'VmLease', Lease), patch.object(repair, 'refresh_worker', side_effect=lambda _: active.append('refresh')), redirect_stdout(io.StringIO()):
                repair.main()
            self.assertEqual(active, ['entered', 'refresh', 'released'])

    @unittest.skipUnless(os.name == 'posix', 'VM lifecycle uses Linux file locks')
    def test_expiring_guard_releases_real_file_lock_and_restarts_idle(self):
        import fcntl
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory); data = root / 'data'; lifecycle = data / 'lifecycle'; lifecycle.mkdir(parents=True)
            lease = root / 'lease'; lease.mkdir(); (lease / 'expires').write_text(str(time.time() + 0.5))
            executable = root / 'systemctl'
            executable.write_text('#!/bin/sh\nprintf "%s\\n" "$1" >> "$MOMSO_TEST_CALLS"\nexit 0\n')
            executable.chmod(0o755)
            env = dict(os.environ, PATH=str(root) + os.pathsep + os.environ['PATH'], MOMSO_TEST_CALLS=str(root / 'calls'))
            guard = GUARD.replace('/var/lib/momso', str(data))
            recover = RECOVER.replace('/var/lib/momso', str(data)).replace('/run/momso-pc-idle-restart', str(root / 'deferred'))
            process = subprocess.Popen([sys.executable, '-c', guard, str(lease)], env=env)
            try:
                deadline = time.monotonic() + 3
                while time.monotonic() < deadline and not (lease / 'status.json').exists(): time.sleep(0.02)
                self.assertEqual(json.loads((lease / 'status.json').read_text())['state'], 'ready')
                with (lifecycle / 'pc-installer.lock').open('a+b') as lock:
                    with self.assertRaises(BlockingIOError): fcntl.flock(lock, fcntl.LOCK_EX | fcntl.LOCK_NB)
                self.assertEqual(process.wait(timeout=8), 0)
                subprocess.run([sys.executable, '-c', recover, str(lease)], env=env, check=True)
                self.assertEqual(json.loads((lease / 'status.json').read_text())['state'], 'released')
                self.assertEqual((root / 'calls').read_text().splitlines(), ['is-active', 'stop', 'start'])
            finally:
                if process.poll() is None: process.terminate(); process.wait(timeout=3)

    @unittest.skipUnless(os.name == 'posix', 'VM lifecycle uses Linux file locks')
    def test_recovery_starts_idle_deferred_by_installer(self):
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory); data = root / 'data'; (data / 'lifecycle').mkdir(parents=True)
            lease = root / 'lease'; lease.mkdir(); (lease / 'was-active').write_text('0')
            deferred = root / 'deferred'; deferred.touch()
            executable = root / 'systemctl'; executable.write_text('#!/bin/sh\nprintf "%s\\n" "$1" >> "$MOMSO_TEST_CALLS"\nexit 0\n'); executable.chmod(0o755)
            env = dict(os.environ, PATH=str(root) + os.pathsep + os.environ['PATH'], MOMSO_TEST_CALLS=str(root / 'calls'))
            recover = RECOVER.replace('/var/lib/momso', str(data)).replace('/run/momso-pc-idle-restart', str(deferred))
            subprocess.run([sys.executable, '-c', recover, str(lease)], env=env, check=True)
            self.assertFalse(deferred.exists())
            self.assertEqual((root / 'calls').read_text().splitlines(), ['start'])

    @unittest.skipUnless(os.name == 'posix' and shutil.which('systemd-run') and shutil.which('sudo'), 'systemd test requires the Linux VM')
    def test_systemd_expiration_runs_the_recovery_hook(self):
        if subprocess.run(['sudo', '-n', 'true'], stdout=subprocess.DEVNULL, stderr=subprocess.DEVNULL).returncode:
            self.skipTest('noninteractive sudo is unavailable')
        with tempfile.TemporaryDirectory(prefix='momso-s154-') as directory:
            root = Path(directory); data = root / 'data'; (data / 'lifecycle').mkdir(parents=True)
            lease = root / 'lease'; lease.mkdir(); (lease / 'expires').write_text(str(time.time() + 1))
            binary = root / 'fake-systemctl'; log = root / 'calls'
            binary.write_text('#!/bin/sh\nprintf "%s\\n" "$1" >> ' + str(log) + '\nexit 0\n'); binary.chmod(0o755)
            def source(code):
                return code.replace('/var/lib/momso', str(data)).replace('/run/momso-pc-idle-restart', str(root / 'deferred')).replace("['systemctl',", '[' + repr(str(binary)) + ',')
            guard = root / 'guard.py'; guard.write_text(source(GUARD))
            recover = root / 'recover.py'; recover.write_text(source(RECOVER))
            unit = 'momso-s154-fixture-' + uuid.uuid4().hex
            command = ['sudo', '-n', 'systemd-run', '--unit=' + unit, '--property=Type=exec', '--property=RuntimeMaxSec=20', '--property=ExecStopPost=/usr/bin/python3 ' + str(recover) + ' ' + str(lease), '--wait', '--collect', '/usr/bin/python3', str(guard), str(lease)]
            subprocess.run(command, check=True, timeout=30, stdout=subprocess.DEVNULL, stderr=subprocess.DEVNULL)
            self.assertEqual(json.loads((lease / 'status.json').read_text())['state'], 'released')
            self.assertEqual(log.read_text().splitlines(), ['is-active', 'stop', 'start'])

    @unittest.skipUnless(os.name == 'posix' and shutil.which('systemd-run') and shutil.which('sudo'), 'systemd test requires the Linux VM')
    def test_periodic_rescue_timer_can_be_created_and_stopped(self):
        if subprocess.run(['sudo', '-n', 'true'], stdout=subprocess.DEVNULL, stderr=subprocess.DEVNULL).returncode:
            self.skipTest('noninteractive sudo is unavailable')
        unit = 'momso-s154-timer-' + uuid.uuid4().hex
        command = ['sudo', '-n', 'systemd-run', '--unit=' + unit, '--on-active=60s', '--on-unit-active=60s', '--collect', '/usr/bin/true']
        subprocess.run(command, check=True, timeout=15, stdout=subprocess.DEVNULL, stderr=subprocess.DEVNULL)
        try:
            self.assertEqual(subprocess.run(['sudo', '-n', 'systemctl', 'is-active', '--quiet', unit + '.timer']).returncode, 0)
        finally:
            subprocess.run(['sudo', '-n', 'systemctl', 'stop', unit + '.timer'], check=True, timeout=15)


if __name__ == '__main__': unittest.main()
