"""Hold the VM's admission lock while the PC removes the owned VM."""
import base64
import json
import re
import time
import uuid
from pathlib import Path

from momso_server_install import InstallError, ZONE


REMOTE_RUNNER = r'''
import json, os, sys, time
from pathlib import Path
root = Path(sys.argv[1])
sys.path.insert(0, str(root))
sys.path.insert(0, '/opt/momso-updater')
def status(value):
    p = root / 'status.tmp'; p.write_text(json.dumps(value), encoding='utf-8'); os.replace(p, root / 'status.json')
try:
    from server_admission import ServerAdmission
    from momso_anywhere_pause_guard import pause_using_server_admission
    admission = ServerAdmission('/run/momso-vault/server-token', '/var/lib/momso/lifecycle')
    def wait_for_deletion():
        status({'state': 'ready'})
        deadline = time.monotonic() + 900
        committed = False
        while True:
            p = root / 'control'
            command = p.read_text(encoding='ascii').strip() if p.exists() else ''
            if command == 'abort': return False
            if command == 'commit' and not committed:
                committed = True; status({'state': 'committed'})
            if not committed and time.monotonic() >= deadline: return False
            time.sleep(1)
    result = pause_using_server_admission(admission, wait_for_deletion)
    reason = result.get('reason')
    uncertain = reason in ('maintenance_release_unconfirmed', 'maintenance_state_unknown')
    status({'state': 'failed' if uncertain else ('released' if not result.get('ok') else 'complete'), 'reason': reason})
except Exception:
    status({'state': 'failed', 'reason': 'pause_unavailable'})
'''


class PcPause:
    def __init__(self, provider, *, sleep=time.sleep, attempts=300):
        self.provider, self.sleep, self.attempts = provider, sleep, attempts
        self.root = '/run/momso-pause-' + provider.install_id + '-' + uuid.uuid4().hex
        self.committed = False

    def _root_python(self, code):
        encoded = base64.b64encode(code.encode('utf-8')).decode('ascii')
        return self.provider.ssh("import base64,subprocess; subprocess.run(['sudo','python3','-c',base64.b64decode(" + repr(encoded) + ").decode()],check=True)")

    def _status(self):
        code = "import pathlib,sys; p=pathlib.Path(" + repr(self.root + '/status.json') + "); sys.stdout.write(p.read_text() if p.is_file() else '{}')"
        raw = self._root_python(code)
        try:
            value = json.loads(raw)
        except (TypeError, ValueError):
            raise InstallError('vm_unreachable') from None
        if not isinstance(value, dict): raise InstallError('vm_unreachable')
        return value

    def _control(self, command):
        if command not in ('commit', 'abort'): raise ValueError('invalid_command')
        self._root_python("import pathlib; p=pathlib.Path(" + repr(self.root + '/control') + "); p.write_text(" + repr(command) + ",encoding='ascii')")

    def start(self):
        launched = False
        try:
            vm = self.provider.run('compute', 'instances', 'describe', self.provider.name, f'--zone={ZONE}', parse=True)
            if vm.get('status') != 'RUNNING' or vm.get('labels', {}).get('momso-install-id') != self.provider.install_id:
                raise InstallError('vm_unreachable')
            guard = (Path(__file__).resolve().parent / 'momso_anywhere_pause_guard.py').read_text(encoding='utf-8')
            payload = ("import os,pathlib,subprocess; p=pathlib.Path(" + repr(self.root) + "); p.mkdir(mode=0o700); "
                       "(p/'momso_anywhere_pause_guard.py').write_text(" + repr(guard) + ",encoding='utf-8'); "
                       "(p/'runner.py').write_text(" + repr(REMOTE_RUNNER) + ",encoding='utf-8'); "
                       "subprocess.Popen(['python3',str(p/'runner.py'),str(p)],stdin=subprocess.DEVNULL,stdout=subprocess.DEVNULL,stderr=subprocess.DEVNULL,start_new_session=True)")
            launched = True
            self._root_python(payload)
            for _ in range(self.attempts):
                state = self._status()
                if state.get('state') == 'ready': return
                if state.get('state') in ('released', 'failed'):
                    reason = state.get('reason', 'backup_freshness_unverified')
                    if reason not in ('backup_not_connected', 'maintenance_release_unconfirmed', 'vm_unreachable'):
                        reason = 'backup_freshness_unverified'
                    raise InstallError(reason)
                self.sleep(1)
            self.abort()
            raise InstallError('backup_freshness_unverified')
        except InstallError as error:
            if str(error) in ('backup_not_connected', 'backup_freshness_unverified', 'maintenance_release_unconfirmed'):
                raise
            if launched:
                try: self.abort()
                except Exception: raise InstallError('vm_unreachable: maintenance_release_unconfirmed') from None
            raise InstallError('vm_unreachable') from None
        except Exception:
            if launched:
                try: self.abort()
                except Exception: raise InstallError('vm_unreachable: maintenance_release_unconfirmed') from None
            raise InstallError('vm_unreachable') from None

    def commit(self):
        try:
            self._control('commit')
            for _ in range(30):
                state = self._status()
                if state.get('state') == 'committed': self.committed = True; return
                if state.get('state') in ('released', 'failed'): break
                self.sleep(1)
        except Exception:
            pass
        self.abort()
        raise InstallError('backup_freshness_unverified')

    def abort(self):
        try:
            self._control('abort')
            for _ in range(45):
                state = self._status()
                if state.get('state') == 'released': return
                if state.get('state') == 'failed': break
                self.sleep(1)
        except Exception:
            pass
        raise InstallError('maintenance_release_unconfirmed')


def guarded_destroy(provider, action, *, guard_factory=PcPause):
    guard = guard_factory(provider)
    guard.start()
    guard.commit()
    try:
        action()
        instances = provider.run('compute', 'instances', 'list', f'--zones={ZONE}', f'--filter=name={provider.name}', parse=True)
        if any(item.get('name') == provider.name for item in instances):
            raise InstallError('vm_deletion_unconfirmed')
    except BaseException:
        try:
            instances = provider.run('compute', 'instances', 'list', f'--zones={ZONE}', f'--filter=name={provider.name}', parse=True)
            matches = [item for item in instances if item.get('name') == provider.name]
            if matches:
                if len(matches) != 1 or matches[0].get('status') != 'RUNNING' or matches[0].get('labels', {}).get('momso-install-id') != provider.install_id:
                    raise InstallError('maintenance_release_unconfirmed')
                guard.abort()
        except Exception:
            raise InstallError('maintenance_release_unconfirmed') from None
        raise
