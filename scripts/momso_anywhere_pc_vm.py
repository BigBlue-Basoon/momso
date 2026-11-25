"""Wake and temporarily protect an owned customer VM during PC operations."""
import base64
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
 if data.is_symlink() or not data.is_dir() or lifecycle.is_symlink():raise RuntimeError('vm_unreachable')
 owner=data.stat()
 if not lifecycle.exists():lifecycle.mkdir(mode=0o700);os.chown(lifecycle,owner.st_uid,owner.st_gid)
 if not lifecycle.is_dir():raise RuntimeError('vm_unreachable')
 lock=os.open(lifecycle/'pc-installer.lock',os.O_CREAT|os.O_RDWR|os.O_NOFOLLOW,0o600)
 os.fchown(lock,owner.st_uid,owner.st_gid)
 try:fcntl.flock(lock,fcntl.LOCK_EX|fcntl.LOCK_NB)
 except BlockingIOError:status({'state':'failed','reason':'vm_busy'});sys.exit(1)
 life=os.open(lifecycle/'lifecycle.lock',os.O_CREAT|os.O_RDWR|os.O_NOFOLLOW,0o600)
 os.fchown(life,owner.st_uid,owner.st_gid)
 deadline=time.monotonic()+30
 while True:
  try:fcntl.flock(life,fcntl.LOCK_EX|fcntl.LOCK_NB);break
  except BlockingIOError:
   if time.monotonic()>=deadline:raise RuntimeError('vm_busy')
   time.sleep(0.25)
 if (lifecycle/'idle-stop.pending').exists():raise RuntimeError('vm_unreachable')
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
 status({'state':'failed','reason':str(error) if str(error) in ('vm_busy','vm_unreachable') else 'vm_unreachable'})
 sys.exit(1)
'''

RECOVER = r'''import fcntl,json,os,pathlib,subprocess,sys,time
root=pathlib.Path(sys.argv[1]);flag=root/'was-active';deferred=pathlib.Path('/run/momso-pc-idle-restart');state='released'
if (flag.is_file() and not flag.is_symlink() and flag.read_text()=='1') or (deferred.is_file() and not deferred.is_symlink()):
 lock=os.open('/var/lib/momso/lifecycle/pc-installer.lock',os.O_CREAT|os.O_RDWR|os.O_NOFOLLOW,0o600)
 deadline=time.monotonic()+60
 while True:
  try:fcntl.flock(lock,fcntl.LOCK_EX|fcntl.LOCK_NB);break
  except BlockingIOError:
   if time.monotonic()>=deadline:state='release_failed';break
   time.sleep(1)
 if state=='released':
  if subprocess.run(['systemctl','start','momso-idle-stop.service'],stdout=subprocess.DEVNULL,stderr=subprocess.DEVNULL,timeout=60).returncode:state='release_failed'
  elif deferred.is_file() and not deferred.is_symlink():deferred.unlink()
 os.close(lock)
p=root/'status.tmp';p.write_text(json.dumps({'state':state}));os.replace(p,root/'status.json')
'''


class VmLease:
    """An expiring systemd guard avoids deadlocking existing lifecycle locks."""
    def __init__(self, provider, *, sleep=time.sleep, attempts=45):
        self.provider, self.sleep, self.attempts = provider, sleep, attempts
        self.token = uuid.uuid4().hex
        self.root = '/run/momso-pc-op-' + self.token
        self.unit = 'momso-pc-op-' + self.token
        self.started = False
        self._stop = threading.Event()
        self._thread = None
        self._renew_failed = False

    def _root(self, code, data=None):
        encoded = base64.b64encode(code.encode()).decode('ascii')
        wrapper = "import base64,subprocess,sys; subprocess.run(['sudo','python3','-c',base64.b64decode(" + repr(encoded) + ").decode()],input=sys.stdin.buffer.read(),check=True)"
        return self.provider.ssh(wrapper, b'' if data is None else data)

    def _status(self):
        code = "import pathlib; p=pathlib.Path(" + repr(self.root + '/status.json') + "); print(p.read_text() if p.is_file() and not p.is_symlink() else '{}')"
        try:
            value = json.loads(self._root(code))
        except (ValueError, TypeError):
            raise InstallError('vm_unreachable') from None
        if not isinstance(value, dict): raise InstallError('vm_unreachable')
        return value

    def __enter__(self):
        ensure_vm_ready(self.provider)
        payload = json.dumps({'guard': GUARD, 'recover': RECOVER}).encode()
        code = r'''import json,os,pathlib,subprocess,sys
root=pathlib.Path(ROOT);root.mkdir(mode=0o700)
data=json.load(sys.stdin)
for name in ('guard','recover'):
 path=root/(name+'.py')
 with path.open('x') as f:f.write(data[name]);f.flush();os.fsync(f.fileno())
import time
with (root/'expires').open('x') as f:f.write(str(time.time()+300));f.flush();os.fsync(f.fileno())
subprocess.run(['systemd-run','--unit='+UNIT+'-rescue','--on-active=300s','--on-unit-active=300s','--collect','/usr/bin/python3',str(root/'recover.py'),str(root)],check=True,stdout=subprocess.DEVNULL,stderr=subprocess.DEVNULL)
subprocess.run(['systemd-run','--unit='+UNIT,'--property=Type=exec','--property=RuntimeMaxSec=86400','--property=ExecStopPost=/usr/bin/python3 '+str(root/'recover.py')+' '+str(root),'--collect','/usr/bin/python3',str(root/'guard.py'),str(root)],check=True,stdout=subprocess.DEVNULL,stderr=subprocess.DEVNULL)
'''.replace('ROOT', repr(self.root)).replace('UNIT', repr(self.unit))
        try:
            self.started = True
            self._root(code, payload)
            for _ in range(self.attempts):
                value = self._status()
                if value.get('state') == 'ready':
                    self._thread = threading.Thread(target=self._heartbeat, daemon=True)
                    self._thread.start()
                    return self
                if value.get('state') == 'failed': raise InstallError(value.get('reason', 'vm_unreachable'))
                self.sleep(1)
            raise InstallError('vm_unreachable')
        except Exception as error:
            try: self.release()
            except (InstallError, OSError, subprocess.SubprocessError): pass
            if isinstance(error, InstallError) and str(error) in ('vm_busy', 'vm_unreachable'):
                raise
            raise InstallError('vm_unreachable') from None

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
        code = "import subprocess; subprocess.run(['systemctl','stop'," + repr(self.unit + '.service') + "],check=True,timeout=90,stdout=subprocess.DEVNULL,stderr=subprocess.DEVNULL)"
        try:
            self._root(code)
            if self._status().get('state') != 'released': raise InstallError('vm_unreachable')
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
            raise InstallError('vm_unreachable') from None
        self.started = False
        if self._renew_failed: raise InstallError('vm_unreachable')

    def __exit__(self, _type, _value, _traceback):
        self.release()
        return False
