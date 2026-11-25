"""Wake and temporarily protect an owned customer VM during PC operations."""
import base64
from contextlib import contextmanager
import json
import subprocess
import threading
import time
import uuid

from momso_server_install import InstallError, ZONE


def existing_vm(provider):
    try:
        rows = provider.run('compute', 'instances', 'list', f'--zones={ZONE}', parse=True, timeout=30)
    except (InstallError, OSError, subprocess.SubprocessError):
        raise InstallError('vm_state_unknown') from None
    if not isinstance(rows, list) or any(not isinstance(row, dict) for row in rows):
        raise InstallError('vm_state_unknown')
    matches = [row for row in rows if row.get('name') == provider.name]
    if not matches: return False
    if len(matches) != 1 or not isinstance(matches[0].get('labels'), dict) or matches[0]['labels'].get('momso-install-id') != provider.install_id:
        raise InstallError('vm_state_unknown')
    return True


def ensure_vm_ready(provider, *, sleep=time.sleep, state_attempts=60, ssh_attempts=24):
    initiated = False
    for _ in range(state_attempts):
        try:
            vm = provider.run('compute', 'instances', 'describe', provider.name,
                              f'--zone={ZONE}', parse=True, timeout=30)
        except (InstallError, OSError, subprocess.SubprocessError):
            raise InstallError('vm_state_unknown') from None
        if not isinstance(vm, dict) or not isinstance(vm.get('labels'), dict) or vm['labels'].get('momso-install-id') != provider.install_id:
            raise InstallError('vm_state_unknown')
        status = vm.get('status')
        if status == 'RUNNING':
            break
        if status in ('TERMINATED', 'STOPPED', 'SUSPENDED') and not initiated:
            action = 'resume' if status == 'SUSPENDED' else 'start'
            try:
                provider.run('compute', 'instances', action, provider.name, f'--zone={ZONE}', timeout=90)
            except (InstallError, OSError, subprocess.SubprocessError):
                raise InstallError('vm_start_failed') from None
            initiated = True
        elif status not in ('TERMINATED', 'STOPPED', 'SUSPENDED', 'PROVISIONING', 'STAGING', 'STARTING', 'STOPPING', 'SUSPENDING', 'REPAIRING'):
            raise InstallError('vm_state_unknown')
        sleep(2)
    else:
        raise InstallError('vm_start_failed' if initiated else 'vm_state_unknown')
    for _ in range(ssh_attempts):
        try:
            provider._ssh_inline('pass', timeout=15)
            return
        except (InstallError, OSError, subprocess.SubprocessError):
            sleep(5)
    raise InstallError('vm_unreachable')


GUARD = r'''import fcntl,json,os,pathlib,subprocess,sys,time
root=pathlib.Path(sys.argv[1]); lifecycle=pathlib.Path('/var/lib/momso/lifecycle')
def status(value):
 p=root/'status.tmp';p.write_text(json.dumps(value));os.replace(p,root/'status.json')
try:
 data=pathlib.Path('/var/lib/momso')
 if data.is_symlink() or not data.is_dir() or lifecycle.is_symlink():raise RuntimeError('vm_layout_invalid')
 owner=data.stat()
 if not lifecycle.exists():lifecycle.mkdir(mode=0o700);os.chown(lifecycle,owner.st_uid,owner.st_gid)
 if not lifecycle.is_dir():raise RuntimeError('vm_layout_invalid')
 lock=os.open(lifecycle/'pc-installer.lock',os.O_CREAT|os.O_RDWR|os.O_NOFOLLOW,0o600)
 os.fchown(lock,owner.st_uid,owner.st_gid)
 try:fcntl.flock(lock,fcntl.LOCK_EX|fcntl.LOCK_NB)
 except BlockingIOError:print('pc_guard_failed reason=vm_busy',file=sys.stderr,flush=True);status({'state':'failed','reason':'vm_busy'});sys.exit(1)
 life=os.open(lifecycle/'lifecycle.lock',os.O_CREAT|os.O_RDWR|os.O_NOFOLLOW,0o600)
 os.fchown(life,owner.st_uid,owner.st_gid)
 deadline=time.monotonic()+30
 while True:
  try:fcntl.flock(life,fcntl.LOCK_EX|fcntl.LOCK_NB);break
  except BlockingIOError:
   if time.monotonic()>=deadline:raise RuntimeError('vm_busy')
   time.sleep(0.25)
 if (lifecycle/'idle-stop.pending').exists():raise RuntimeError('idle_stop_recovering')
 active=subprocess.run(['systemctl','is-active','--quiet','momso-idle-stop.service'],stdout=subprocess.DEVNULL,stderr=subprocess.DEVNULL).returncode==0
 p=root/'was-active'
 with p.open('x') as f:f.write('1' if active else '0');f.flush();os.fsync(f.fileno())
 if active:subprocess.run(['systemctl','stop','momso-idle-stop.service'],check=True,timeout=60,stdout=subprocess.DEVNULL,stderr=subprocess.DEVNULL)
 fcntl.flock(life,fcntl.LOCK_UN);os.close(life)
 status({'state':'ready'})
 while True:
  p=root/'expires'
  if p.is_symlink() or not p.is_file() or time.time()>=float(p.read_text()):break
  time.sleep(5)
  if time.time()>=float(p.read_text()):break
  if subprocess.run(['systemctl','is-active','--quiet','momso-idle-stop.service'],stdout=subprocess.DEVNULL,stderr=subprocess.DEVNULL,timeout=10).returncode==0:
   with (root/'was-active').open('w') as f:f.write('1');f.flush();os.fsync(f.fileno())
   subprocess.run(['systemctl','stop','momso-idle-stop.service'],check=True,timeout=60,stdout=subprocess.DEVNULL,stderr=subprocess.DEVNULL)
except Exception as error:
 reason=str(error) if str(error) in ('vm_busy','vm_layout_invalid','idle_stop_recovering','idle_stop_stop_failed') else 'idle_stop_stop_failed' if isinstance(error,subprocess.SubprocessError) else 'pc_guard_failed'
 print('pc_guard_failed reason='+reason,file=sys.stderr,flush=True)
 status({'state':'failed','reason':reason})
 sys.exit(1)
'''

RECOVER = r'''import fcntl,json,os,pathlib,subprocess,sys,time
root=pathlib.Path(sys.argv[1]);flag=root/'was-active';deferred=pathlib.Path('/run/momso-pc-idle-restart');state='released'
try:previous=json.loads((root/'status.json').read_text())
except Exception:previous={}
if not isinstance(previous,dict):previous={}
if (flag.is_file() and not flag.is_symlink() and flag.read_text()=='1') or (deferred.is_file() and not deferred.is_symlink()):
 lock=os.open('/var/lib/momso/lifecycle/pc-installer.lock',os.O_CREAT|os.O_RDWR|os.O_NOFOLLOW,0o600)
 deadline=time.monotonic()+60
 while True:
  try:fcntl.flock(lock,fcntl.LOCK_EX|fcntl.LOCK_NB);break
  except BlockingIOError:
   if time.monotonic()>=deadline:state='release_failed';break
   time.sleep(1)
 if state=='release_failed' and previous.get('state')=='ready' and subprocess.run(['systemctl','is-active','--quiet',UNIT_NAME],stdout=subprocess.DEVNULL,stderr=subprocess.DEVNULL,timeout=10).returncode==0:
  os.close(lock);sys.exit(0)
 if state=='released':
  if subprocess.run(['systemctl','start','momso-idle-stop.service'],stdout=subprocess.DEVNULL,stderr=subprocess.DEVNULL,timeout=60).returncode:state='release_failed'
  elif deferred.is_file() and not deferred.is_symlink():deferred.unlink()
 os.close(lock)
out={'state':state}
if previous.get('state')=='failed' or previous.get('guard')=='failed':
 out['guard']='failed';out['reason']=previous.get('reason') if previous.get('reason') in ('vm_busy','vm_layout_invalid','idle_stop_recovering','idle_stop_stop_failed','pc_guard_failed') else 'pc_guard_failed'
p=root/'status.tmp';p.write_text(json.dumps(out));os.replace(p,root/'status.json')
'''


GUARD_REASONS = ('vm_busy', 'vm_layout_invalid', 'idle_stop_recovering', 'idle_stop_stop_failed', 'pc_guard_failed')
RETRY_REASONS = ('vm_busy', 'idle_stop_recovering')
KNOWN = GUARD_REASONS + ('vm_unreachable', 'vm_state_unknown', 'vm_start_failed', 'pc_guard_ended',
         'pc_guard_status_invalid', 'pc_guard_command_failed', 'pc_guard_not_ready', 'pc_guard_release_unconfirmed')


@contextmanager
def vm_restart(provider):
    """Wrap a VM stop/start; a PC guard held over it is suspended and renewed."""
    lease = getattr(provider, '_vm_lease', None)
    if lease is not None:
        lease.suspend()
    yield
    if lease is not None:
        lease.resume()


class VmLease:
    """An expiring systemd guard avoids deadlocking existing lifecycle locks.

    The VM's own updater holds lifecycle.lock while it installs. The guard then
    ends with vm_busy; the PC waits and starts a fresh guard (S179) instead of
    polling a guard that has already ended.
    """
    def __init__(self, provider, *, sleep=time.sleep, attempts=45, busy_attempts=40, busy_wait=15):
        self.provider, self.sleep, self.attempts = provider, sleep, attempts
        self.busy_attempts, self.busy_wait = busy_attempts, busy_wait
        self.started = False
        self._new_token()

    def _current_boot(self):
        try:
            raw = self.provider._ssh_inline("print(open('/proc/sys/kernel/random/boot_id').read().strip())", timeout=30)
        except (InstallError, OSError, subprocess.SubprocessError):
            return None
        value = raw.decode('ascii', 'replace').strip() if isinstance(raw, bytes) else str(raw or '').strip()
        return value if len(value) == 36 else None

    def suspend(self):
        """Release before the VM is stopped; /run and the guard do not survive it."""
        self.release()
        self._suspended = True

    def resume(self):
        """Start a fresh guard after the VM came back (S179c)."""
        self._suspended = False
        self._new_token()
        ensure_vm_ready(self.provider)
        self._enter_with_retries()
        self.provider._vm_lease = self

    def _new_token(self):
        # A fresh guard needs a fresh heartbeat state; release() set _stop.
        self._stop = threading.Event()
        self._thread = None
        self._renew_failed = False
        self.token = uuid.uuid4().hex
        self.root = '/run/momso-pc-op-' + self.token
        self.unit = 'momso-pc-op-' + self.token

    def _root(self, code, data=None):
        encoded = base64.b64encode(code.encode()).decode('ascii')
        wrapper = "import base64,subprocess,sys; subprocess.run(['sudo','python3','-c',base64.b64decode(" + repr(encoded) + ").decode()],input=sys.stdin.buffer.read(),check=True)"
        return self.provider.ssh(wrapper, b'' if data is None else data)

    def _status(self):
        code = "import pathlib; p=pathlib.Path(" + repr(self.root + '/status.json') + "); print(p.read_text() if p.is_file() and not p.is_symlink() else '{}')"
        try:
            raw = self._root(code)
        except (InstallError, OSError, subprocess.SubprocessError):
            # SSH answered but the remote command failed: not "unreachable".
            raise InstallError('pc_guard_command_failed') from None
        try:
            value = json.loads(raw)
        except (ValueError, TypeError):
            raise InstallError('pc_guard_status_invalid') from None
        if not isinstance(value, dict): raise InstallError('pc_guard_status_invalid')
        return value

    def __enter__(self):
        ensure_vm_ready(self.provider)
        self._enter_with_retries()
        # Steps that stop and start the VM find the lease here (vm_restart).
        self.provider._vm_lease = self
        return self

    def _enter_with_retries(self):
        for attempt in range(self.busy_attempts):
            try:
                return self._start_guard()
            except InstallError as error:
                reason = str(error)
                if reason not in RETRY_REASONS or attempt == self.busy_attempts - 1:
                    raise
            # The VM is busy (its own updater) or the idle stop is still
            # reopening admission after the last stop: wait, then a new guard.
            print('pc_guard_waiting reason=' + reason, flush=True)
            self._new_token()
            self.sleep(self.busy_wait)
        raise InstallError('vm_busy')

    def _start_guard(self):
        payload = json.dumps({'guard': GUARD, 'recover': RECOVER.replace('UNIT_NAME', repr(self.unit + '.service'))}).encode()
        code = r'''import json,os,pathlib,subprocess,sys
root=pathlib.Path(ROOT);root.mkdir(mode=0o700)
data=json.load(sys.stdin)
for name in ('guard','recover'):
 path=root/(name+'.py')
 with path.open('x') as f:f.write(data[name]);f.flush();os.fsync(f.fileno())
import time
with (root/'expires').open('x') as f:f.write(str(time.time()+300));f.flush();os.fsync(f.fileno())
print(open('/proc/sys/kernel/random/boot_id').read().strip())
subprocess.run(['systemd-run','--unit='+UNIT+'-rescue','--on-active=300s','--on-unit-active=300s','--collect','/usr/bin/python3',str(root/'recover.py'),str(root)],check=True,stdout=subprocess.DEVNULL,stderr=subprocess.DEVNULL)
subprocess.run(['systemd-run','--unit='+UNIT,'--property=Type=exec','--property=RuntimeMaxSec=86400','--property=ExecStopPost=/usr/bin/python3 '+str(root/'recover.py')+' '+str(root),'--collect','/usr/bin/python3',str(root/'guard.py'),str(root)],check=True,stdout=subprocess.DEVNULL,stderr=subprocess.DEVNULL)
'''.replace('ROOT', repr(self.root)).replace('UNIT', repr(self.unit))
        try:
            self.started = True
            boot = self._root(code, payload)
            boot = boot.decode('ascii', 'replace').strip() if isinstance(boot, bytes) else str(boot or '').strip()
            self.boot_id = boot if len(boot) == 36 else None
            for _ in range(self.attempts):
                value = self._status()
                if value.get('state') == 'ready':
                    self._thread = threading.Thread(target=self._heartbeat, daemon=True)
                    self._thread.start()
                    return self
                if value.get('state') == 'failed' or value.get('guard') == 'failed':
                    raise InstallError(value.get('reason') if value.get('reason') in GUARD_REASONS else 'pc_guard_failed')
                if value.get('state') in ('released', 'release_failed'):
                    # The guard ended before it was ready; polling more cannot help.
                    raise InstallError('pc_guard_ended')
                self.sleep(1)
            raise InstallError('pc_guard_not_ready')
        except Exception as error:
            try: self.release()
            except (InstallError, OSError, subprocess.SubprocessError): pass
            self.started = False
            if isinstance(error, InstallError) and str(error) in KNOWN:
                raise
            raise InstallError('pc_guard_command_failed') from None

    def _heartbeat(self):
        while not self._stop.wait(60):
            code = "import os,pathlib,subprocess,time; root=pathlib.Path(" + repr(self.root) + "); subprocess.run(['systemctl','is-active','--quiet'," + repr(self.unit + '.service') + "],check=True,timeout=10); p=root/'expires.tmp'; fd=os.open(p,os.O_WRONLY|os.O_CREAT|os.O_TRUNC|os.O_NOFOLLOW,0o600); f=os.fdopen(fd,'w'); f.write(str(time.time()+300)); f.flush(); os.fsync(f.fileno()); f.close(); os.replace(p,root/'expires')"
            encoded = base64.b64encode(code.encode()).decode('ascii')
            wrapper = "import base64,subprocess; subprocess.run(['sudo','python3','-c',base64.b64decode(" + repr(encoded) + ").decode()],check=True,timeout=15)"
            try:
                self.provider._ssh_inline(wrapper, timeout=20)
                self._renew_failed = False
            except (InstallError, OSError, subprocess.SubprocessError):
                self._renew_failed = True

    def release(self):
        if not self.started: return
        self._stop.set()
        if self._thread is not None: self._thread.join(timeout=25)
        # Exit 5 = unit not loaded: the guard already ended (and its stop hook ran).
        code = "import subprocess; r=subprocess.run(['systemctl','stop'," + repr(self.unit + '.service') + "],timeout=90,stdout=subprocess.DEVNULL,stderr=subprocess.DEVNULL); assert r.returncode in (0,5)"
        boot = getattr(self, 'boot_id', None)
        if boot is not None and self._current_boot() not in (None, boot):
            # The VM restarted since the guard began: /run (guard, status, timer)
            # is gone and idle stop starts with the new boot. Nothing to release.
            self.started = False
            self._stop.set()
            if getattr(self.provider, '_vm_lease', None) is self: self.provider._vm_lease = None
            return
        try:
            self._root(code)
            if self._status().get('state') != 'released': raise InstallError('pc_guard_release_unconfirmed')
            cleanup = "import os,pathlib,subprocess; timer=" + repr(self.unit + '-rescue.timer') + "; service=" + repr(self.unit + '-rescue.service') + "; subprocess.run(['systemctl','stop',timer],check=False,timeout=30,stdout=subprocess.DEVNULL,stderr=subprocess.DEVNULL); assert subprocess.run(['systemctl','is-active','--quiet',timer],stdout=subprocess.DEVNULL,stderr=subprocess.DEVNULL,timeout=10).returncode in (3,4); subprocess.run(['systemctl','stop',service],check=False,timeout=90,stdout=subprocess.DEVNULL,stderr=subprocess.DEVNULL); assert subprocess.run(['systemctl','is-active','--quiet',service],stdout=subprocess.DEVNULL,stderr=subprocess.DEVNULL,timeout=10).returncode in (3,4); root=pathlib.Path(" + repr(self.root) + "); assert root.parent==pathlib.Path('/run') and root.name==" + repr('momso-pc-op-' + self.token) + "; [(root/name).unlink(missing_ok=True) for name in ('guard.py','recover.py','was-active','status.json','status.tmp','expires','expires.tmp')]; root.rmdir()"
            self._root(cleanup)
        except (InstallError, OSError, subprocess.SubprocessError):
            try:
                rows = self.provider.run('compute', 'instances', 'list', f'--zones={ZONE}', parse=True, timeout=30)
                if isinstance(rows, list) and not any(row.get('name') == self.provider.name for row in rows):
                    self.started = False
                    return
            except (InstallError, OSError, subprocess.SubprocessError):
                pass
            raise InstallError('pc_guard_release_unconfirmed') from None
        self.started = False
        if getattr(self.provider, '_vm_lease', None) is self and not getattr(self, '_suspended', False):
            self.provider._vm_lease = None
        if self._renew_failed:
            # The release itself was confirmed; report the gap without failing.
            print('pc_guard_warning reason=renew_failed', flush=True)

    def __exit__(self, _type, _value, _traceback):
        try:
            return self._exit(_type)
        finally:
            if getattr(self.provider, '_vm_lease', None) is self:
                self.provider._vm_lease = None

    def _exit(self, _type):
        if _type is None:
            self.release()
            return False
        # Never replace the operation's own error with a release failure.
        try:
            self.release()
        except (InstallError, OSError, subprocess.SubprocessError) as error:
            print('pc_guard_warning reason=' + (str(error) if str(error) in KNOWN else 'pc_guard_release_unconfirmed'), flush=True)
        return False
