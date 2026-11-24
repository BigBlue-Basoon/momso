"""Provision a private momso server VM without using gcloud defaults."""
from __future__ import annotations

import argparse
import base64
import hashlib
import json
import os
import re
import shutil
import stat
import subprocess
import sys
import tempfile
import tarfile
import time
import uuid
from pathlib import Path

if __name__ == '__main__':
    sys.modules.setdefault('momso_server_install', sys.modules[__name__])

ZONE = "asia-northeast3-a"
REGION = "asia-northeast3"
MACHINE = "e2-small"


class InstallError(Exception):
    def __init__(self, message, *, role_propagating=False, managed_conflict=None, managed_conflicts=None, gateway_failure=None, gateway_code=None):
        super().__init__(message)
        self.role_propagating = role_propagating
        self.managed_conflict = managed_conflict
        self.managed_conflicts = managed_conflicts or ([managed_conflict] if managed_conflict else [])
        self.gateway_failure = gateway_failure
        self.gateway_code = gateway_code


class PendingNetworkCleanup(InstallError):
    def __init__(self, resources):
        self.resources = resources
        super().__init__("네트워크 정리가 아직 끝나지 않았습니다. Cloud Run 주소 회수 지연 또는 권한 문제일 수 있습니다. 조금 뒤 같은 삭제 명령을 다시 실행하세요. 남은 자원: " + ", ".join(resources))


def save_state(path: Path, state: dict) -> None:
    path.parent.mkdir(parents=True, exist_ok=True)
    fd, name = tempfile.mkstemp(prefix=".momso-install-", dir=path.parent)
    try:
        with os.fdopen(fd, "w", encoding="utf-8") as out:
            json.dump(state, out, ensure_ascii=False)
            out.flush()
            os.fsync(out.fileno())
        os.replace(name, path)
    finally:
        if os.path.exists(name):
            os.unlink(name)


def initialize_state(path: Path, account: str, project: str) -> dict:
    if path.exists():
        state = json.loads(path.read_text(encoding="utf-8"))
        if state.get("version") != 1 or state.get("account") != account or state.get("project") != project or not re.fullmatch(r"[0-9a-f]{12}", state.get("id", "")):
            raise InstallError("설치 기록이 계정이나 프로젝트와 맞지 않습니다.")
        return state
    state = {"version": 1, "id": uuid.uuid4().hex[:12], "account": account, "project": project, "steps": {}}
    save_state(path, state)
    return state


def is_link_or_reparse(path: Path) -> bool:
    if path.is_symlink():
        return True
    try:
        return bool(getattr(path.lstat(), "st_file_attributes", 0) & getattr(stat, "FILE_ATTRIBUTE_REPARSE_POINT", 0x400))
    except FileNotFoundError:
        return False


def key_directory(state_path: Path, install_id: str, create: bool) -> Path:
    directory = state_path.resolve().parent / f".momso-ssh-{install_id}"
    marker = directory / "owner"
    if is_link_or_reparse(directory):
        raise InstallError("설치 키 폴더가 안전한 폴더가 아닙니다.")
    if not directory.exists():
        if not create:
            return directory
        directory.mkdir(mode=0o700)
    if not directory.is_dir():
        raise InstallError("설치 키 폴더를 열 수 없습니다.")
    if not marker.exists() and create and not any(directory.iterdir()):
        marker.write_text(install_id, encoding="ascii")
    if is_link_or_reparse(marker) or not marker.is_file() or marker.read_text(encoding="ascii") != install_id:
        raise InstallError("설치 키 폴더의 소유자를 확인하지 못했습니다.")
    return directory


def cleanup_keys(directory: Path, install_id: str) -> None:
    if not directory.exists():
        return
    marker = directory / "owner"
    if is_link_or_reparse(directory) or is_link_or_reparse(marker) or not marker.is_file() or marker.read_text(encoding="ascii") != install_id:
        raise InstallError("설치 키 폴더의 소유자를 확인하지 못했습니다.")
    for name in ("ssh", "ssh.pub"):
        path = directory / name
        if is_link_or_reparse(path) or (path.exists() and not path.is_file()):
            raise InstallError("설치 키 파일이 안전한 파일이 아니므로 남겨 두었습니다.")
        if path.exists():
            path.unlink()
    if {entry.name for entry in directory.iterdir()} == {"owner"}:
        marker.unlink()
        directory.rmdir()


class GoogleProvider:
    def __init__(self, account: str, project: str, install_id: str, key_file: Path):
        self.account, self.project, self.install_id = account, project, install_id
        self.name = f"momso-{install_id}"
        self.network = f"{self.name}-net"
        self.subnet = f"{self.name}-sub"
        self.firewall = f"{self.name}-iap"
        self.router = f"{self.name}-router"
        self.nat = f"{self.name}-nat"
        self.marker = f"momso-install-{install_id}"
        self.key_file = key_file
        self.gcloud = shutil.which("gcloud") or shutil.which("gcloud.cmd")
        if not self.gcloud:
            raise InstallError("Google Cloud CLI를 찾지 못했습니다.")

    def run(self, *args: str, data: bytes | None = None, parse: bool = False):
        prefix = [self.gcloud]
        env = os.environ.copy()
        if os.name == "nt" and self.gcloud.lower().endswith((".cmd", ".bat")):
            root = Path(self.gcloud).resolve().parent.parent
            entry = root / "lib" / "gcloud.py"
            bundled = root / "platform" / "bundledpython" / "python.exe"
            if not entry.is_file():
                raise InstallError("Google Cloud CLI의 실행 파일을 찾지 못했습니다.")
            prefix = [str(bundled) if bundled.is_file() else sys.executable, "-S", str(entry)]
            env["CLOUDSDK_ROOT_DIR"] = str(root)
            env["PATH"] = str(root / "bin" / "sdk") + os.pathsep + env.get("PATH", "")
            env.pop("PYTHONHOME", None)
        command = [*prefix, *args, f"--account={self.account}", f"--project={self.project}", "--quiet"]
        if parse:
            command.append("--format=json")
        if args[:2] == ('compute', 'ssh') and len(subprocess.list2cmdline(command)) > 8000:
            raise InstallError("VM 접속 명령이 너무 깁니다.")
        flags = getattr(subprocess, "CREATE_NO_WINDOW", 0) if os.name == "nt" else 0
        result = subprocess.run(command, input=data, capture_output=True, timeout=600, check=False, env=env, creationflags=flags)
        if result.returncode:
            from momso_iam_retry import role_propagation_failure
            stderr = getattr(result, 'stderr', b'') or b''
            matches = re.findall(rb'momso-managed-conflict: ([a-z0-9_.-]{1,80}): (unrecognized|not_regular|invalid_name|manifest_invalid|directory_untrusted)', stderr)
            if matches:
                from momso_managed_history import load
                history = load()
                allowed = set().union(*(set(history[group]) for group in ('files', 'worker', 'proxy')))
                allowed.update(('unknown', 'idle-token.py', 'momso-vault-credential.service', 'momso-idle-stop.service', 'vault.conf', 'backup.conf', 'momso-update-public.pem', 'managed-files.json', 'managed-helper.json', 'managed-units.json', 'managed-overrides.json', 'momso', 'system', 'momso-private', 'momso-server.service.d'))
                conflicts = list(dict.fromkeys((name.decode('ascii'), reason.decode('ascii')) for name, reason in matches if name.decode('ascii') in allowed))
                if conflicts:
                    raise InstallError('\n'.join(f"관리 파일 충돌: {name}: {reason}" for name, reason in conflicts), managed_conflict=conflicts[0], managed_conflicts=conflicts)
            raise InstallError(f"클라우드 작업 실패: {' '.join(args[:3])}. 권한·연결·자원 상태를 확인하세요.",
                               role_propagating=role_propagation_failure(stderr),
                               gateway_code='gateway_image_permission' if args[:2] == ('run', 'deploy') and (b'artifactregistry.repositories.downloadArtifacts' in stderr or b'permission to read the image' in stderr.lower()) else None)
        if parse:
            try:
                return json.loads(result.stdout.decode("utf-8"))
            except (UnicodeDecodeError, json.JSONDecodeError) as error:
                raise InstallError("클라우드 응답을 읽지 못했습니다.") from error
        return result.stdout

    def ensure(self, kind: str, name: str, list_args: list[str], create_args: list[str], vm: bool = False):
        items = self.run(*list_args, f"--filter=name={name}", parse=True)
        found = next((item for item in items if item.get("name") == name), None)
        if found:
            owned = found.get("labels", {}).get("momso-install-id") == self.install_id if vm else found.get("description") == self.marker
            if not owned:
                raise InstallError(f"{kind} 이름이 기존 자원과 겹칩니다. 기존 자원은 변경하지 않았습니다.")
            return
        self.run(*create_args)

    def prepare_network(self):
        self.ensure("네트워크", self.network, ["compute", "networks", "list"], ["compute", "networks", "create", self.network, "--subnet-mode=custom", f"--description={self.marker}"])
        self.ensure("서브넷", self.subnet, ["compute", "networks", "subnets", "list", f"--regions={REGION}"], ["compute", "networks", "subnets", "create", self.subnet, f"--network={self.network}", f"--region={REGION}", "--range=10.42.0.0/24", f"--description={self.marker}"])
        self.ensure("방화벽", self.firewall, ["compute", "firewall-rules", "list"], ["compute", "firewall-rules", "create", self.firewall, f"--network={self.network}", "--direction=INGRESS", "--action=ALLOW", "--rules=tcp:22", "--source-ranges=35.235.240.0/20", f"--target-tags={self.name}", f"--description={self.marker}"])
        self.ensure("라우터", self.router, ["compute", "routers", "list", f"--regions={REGION}"], ["compute", "routers", "create", self.router, f"--network={self.network}", f"--region={REGION}", f"--description={self.marker}"])
        router = self.run("compute", "routers", "describe", self.router, f"--region={REGION}", parse=True)
        if not any(nat.get("name") == self.nat for nat in router.get("nats", [])):
            self.run("compute", "routers", "nats", "create", self.nat, f"--router={self.router}", f"--region={REGION}", "--nat-all-subnet-ip-ranges", "--auto-allocate-nat-external-ips")

    def prepare_vm(self):
        self.ensure("VM", self.name, ["compute", "instances", "list", f"--zones={ZONE}"], ["compute", "instances", "create", self.name, f"--zone={ZONE}", f"--machine-type={MACHINE}", f"--subnet={self.subnet}", "--no-address", "--no-service-account", "--no-scopes", "--image-family=debian-12", "--image-project=debian-cloud", "--boot-disk-size=20GB", f"--tags={self.name}", f"--labels=momso-install-id={self.install_id}", "--metadata=enable-oslogin=FALSE,block-project-ssh-keys=TRUE,enable-guest-attributes=TRUE"], vm=True)
        vm = self.run("compute", "instances", "describe", self.name, f"--zone={ZONE}", parse=True)
        interfaces = vm.get("networkInterfaces", [])
        if vm.get("labels", {}).get("momso-install-id") != self.install_id or not interfaces or any(item.get("accessConfigs") or item.get("ipv6AccessConfigs") or not item.get("network", "").endswith("/" + self.network) for item in interfaces):
            raise InstallError("VM의 소유권이나 사설 네트워크 설정을 확인하지 못했습니다.")
        if vm.get("status") in ("TERMINATED", "SUSPENDED"):
            self.run("compute", "instances", "start", self.name, f"--zone={ZONE}")

    def ssh(self, code: str, data: bytes | None = None):
        remote_dir = f"/tmp/{self.name}-transfer-{uuid.uuid4().hex}"
        remote = remote_dir + "/payload"
        code_bytes = code.encode('utf-8')
        package = len(code_bytes).to_bytes(8, 'big') + (b'1' if data is not None else b'0') + code_bytes + (data or b'')
        fd, local = tempfile.mkstemp(prefix="momso-transfer-")
        created = False
        try:
            with os.fdopen(fd, "wb") as output:
                output.write(package)
            self._ssh_inline(f"import os; os.mkdir({remote_dir!r}, 0o700)")
            created = True
            self.run("compute", "scp", local, f"{self.name}:{remote}", f"--zone={ZONE}", "--tunnel-through-iap", "--strict-host-key-checking=yes", f"--ssh-key-file={self.key_file}")
            digest = hashlib.sha256(package).hexdigest()
            wrapper = (
                "import hashlib,io,os,sys\n"
                f"p={remote!r}\n"
                "with open(p,'rb') as f: d=f.read()\n"
                "os.unlink(p)\n"
                f"os.rmdir({remote_dir!r})\n"
                f"assert hashlib.sha256(d).hexdigest()=={digest!r}\n"
                "n=int.from_bytes(d[:8],'big'); flag=d[8:9]; code=d[9:9+n]\n"
                "assert flag in (b'0',b'1') and len(code)==n\n"
                "if flag==b'1': sys.stdin=io.TextIOWrapper(io.BytesIO(d[9+n:]),encoding='utf-8')\n"
                "exec(compile(code,'<momso>','exec'))\n"
            )
            return self._ssh_inline(wrapper)
        except Exception:
            if created:
                cleanup = f"import os; p={remote!r}; d={remote_dir!r}; os.path.exists(p) and os.unlink(p); os.path.isdir(d) and os.rmdir(d)"
                try:
                    self._ssh_inline(cleanup)
                except (InstallError, subprocess.SubprocessError, OSError):
                    pass
            raise
        finally:
            os.unlink(local)

    def _ssh_inline(self, code: str):
        encoded = base64.b64encode(code.encode()).decode("ascii")
        command = f"python3 -c \"exec(__import__('base64').b64decode('{encoded}'))\""
        return self.run("compute", "ssh", self.name, f"--zone={ZONE}", "--tunnel-through-iap", "--strict-host-key-checking=yes", f"--ssh-key-file={self.key_file}", f"--command={command}")

    def install(self, artifact: Path, digest: str):
        data = artifact.read_bytes()
        if hashlib.sha256(data).hexdigest() != digest:
            raise InstallError("설치 파일의 SHA-256이 지정한 값과 다릅니다.")
        transfer = "import hashlib,os,sys,tempfile\nd=sys.stdin.buffer.read()\nassert hashlib.sha256(d).hexdigest()==" + repr(digest) + "\np='/tmp/" + self.name + "-server'\nf=open(p+'.tmp','wb');f.write(d);f.flush();os.fsync(f.fileno());f.close();os.replace(p+'.tmp',p)\n"
        for attempt in range(12):
            try:
                self.ssh(transfer, data)
                break
            except (InstallError, subprocess.TimeoutExpired):
                if attempt == 11:
                    raise InstallError("IAP 접속 또는 파일 전달에 실패했습니다. 권한과 VM 준비 상태를 확인한 뒤 다시 실행하세요.")
                time.sleep(10)
        payload = """import hashlib,os,shutil,subprocess,sys
src=SOURCE
assert hashlib.sha256(open(src,'rb').read()).hexdigest()==DIGEST
def run(*args): subprocess.run(args,check=True,stdout=subprocess.DEVNULL)
missing=[name for name in ('git','ffmpeg') if not shutil.which(name)]
if missing:
 os.environ['DEBIAN_FRONTEND']='noninteractive';run('apt-get','update','-qq');run('apt-get','install','-y','-qq',*missing)
if subprocess.run(['id','momso'],stdout=subprocess.DEVNULL,stderr=subprocess.DEVNULL).returncode: run('useradd','--system','--create-home','--home-dir','/var/lib/momso','--shell','/usr/sbin/nologin','momso')
run('install','-d','-m','0755','/opt/momso')
run('install','-m','0755',src,'/opt/momso/.momso-server.new')
os.replace('/opt/momso/.momso-server.new','/opt/momso/momso-server')
env='/etc/momso-server.env'
if not os.path.exists(env):
 import secrets
 fd=os.open(env,os.O_WRONLY|os.O_CREAT|os.O_EXCL,0o600)
 with os.fdopen(fd,'w') as f: f.write('MOMSO_SERVER_TOKEN='+secrets.token_urlsafe(32)+'\\nMOMSO_SERVER_DATA_DIR=/var/lib/momso\\nMOMSO_SERVER_LISTEN=127.0.0.1:8765\\n')
unit='[Unit]\\nDescription=momso server\\nAfter=network-online.target\\nWants=network-online.target\\n[Service]\\nUser=momso\\nEnvironmentFile=/etc/momso-server.env\\nExecStart=/opt/momso/momso-server\\nRestart=on-failure\\n[Install]\\nWantedBy=multi-user.target\\n'
open('/etc/systemd/system/momso-server.service','w').write(unit)
run('systemctl','daemon-reload');run('systemctl','enable','--now','momso-server');run('systemctl','restart','momso-server');os.remove(src)
""".replace("SOURCE", repr(f"/tmp/{self.name}-server")).replace("DIGEST", repr(digest))
        wrapped = "import base64,subprocess\np=" + repr(base64.b64encode(payload.encode()).decode()) + "\nsubprocess.run(['sudo','python3','-c',__import__('base64').b64decode(p).decode()],check=True)\n"
        self.ssh(wrapped)

    def health(self):
        code = """import subprocess,sys
code='import os,urllib.request,json; t=open(\"/etc/momso-server.env\").read().splitlines()[0].split(\"=\",1)[1]; r=urllib.request.Request(\"http://127.0.0.1:8765/health\",headers={\"Authorization\":\"Bearer \"+t}); assert json.load(urllib.request.urlopen(r,timeout=5))[\"state\"]==\"ready\"'
subprocess.run(['systemctl','is-active','--quiet','momso-server'],check=True)
subprocess.run(['sudo','python3','-c',code],check=True)
"""
        for attempt in range(12):
            try:
                self.ssh(code)
                return
            except (InstallError, subprocess.TimeoutExpired):
                if attempt == 11:
                    raise InstallError("서비스 상태를 확인하지 못했습니다. VM 안의 서비스 기록을 확인하세요.")
                time.sleep(5)

    def ensure_private_proxy(self):
        code = r'''import json,ssl,subprocess,time,urllib.error,urllib.request
subprocess.run(['sudo','systemctl','start','momso-private-proxy'],check=True,stdout=subprocess.DEVNULL,stderr=subprocess.DEVNULL)
check=r"""import json,ssl,urllib.request
with open('/etc/momso-private/transport-token') as f:token=f.read().strip()
ctx=ssl.create_default_context(cafile='/etc/momso-private/certificate.pem');ctx.check_hostname=False
req=urllib.request.Request('https://127.0.0.1:8766/health',headers={'Authorization':'Bearer '+token})
with urllib.request.urlopen(req,context=ctx,timeout=3) as response:assert json.load(response).get('state')=='ready'
"""
for attempt in range(12):
 if subprocess.run(['sudo','python3','-c',check],stdout=subprocess.DEVNULL,stderr=subprocess.DEVNULL).returncode==0:break
 if attempt==11:raise RuntimeError('private_proxy_unavailable')
 time.sleep(1)
'''
        try:
            self.ssh(code)
        except (InstallError, OSError, subprocess.SubprocessError) as error:
            raise InstallError("입구 연결을 다시 확인하지 못했습니다. 서버와 입구 상태를 확인하세요.") from error

    def upgrade_checkpoint(self, action):
        if action not in ("begin", "restore", "release", "finish"):
            raise ValueError("invalid upgrade action")
        snapshots = Path(__file__).resolve().parent.parent / 'apps/alpha/desktop/src-tauri/momso-server-update/data_snapshots.py'
        snapshot_code = base64.b64encode(snapshots.read_bytes()).decode('ascii')
        managed_code = base64.b64encode((Path(__file__).resolve().parent / 'momso_managed_files.py').read_bytes()).decode('ascii')
        code = r'''import base64,fcntl,json,os,shutil,subprocess,tempfile,time,urllib.error,urllib.request
root='/opt/momso';backup=root+'/.momso-upgrade-backup';env='/etc/momso-server.env'
marker=root+'/releases/.momso-update-active';current=root+'/releases/current';override='/etc/systemd/system/momso-server.service.d/momso-update.conf'
active=os.path.exists(marker) or (os.path.isfile(override) and 'ExecStart=/opt/momso/releases/current/momso-server' in open(override).read())
if active:
 assert (not os.path.exists(marker) or (not os.path.islink(marker) and open(marker).read().strip()=='momso-update-v1')) and os.path.islink(current)
 target=os.path.realpath(current);assert os.path.commonpath((target,root+'/releases'))==root+'/releases' and target!=root+'/releases'
 root=target
server=root+'/momso-server';web=root+'/web'
action=ACTION
ns={};exec(compile(base64.b64decode(SNAPSHOT_CODE),'data_snapshots.py','exec'),ns);DataSnapshots=ns['DataSnapshots']
config=dict(line.strip().split('=',1) for line in open(env) if '=' in line and not line.lstrip().startswith('#'))
data=config['MOMSO_SERVER_DATA_DIR'];token=config['MOMSO_SERVER_TOKEN']
assert os.path.isabs(data) and data!='/var/lib' and not os.path.islink(data)
def call(path, method='GET'):
 req=urllib.request.Request('http://127.0.0.1:8765'+path,headers={'Authorization':'Bearer '+token},method=method,data=b'' if method=='POST' else None)
 with urllib.request.urlopen(req,timeout=5) as response:return json.load(response)
def owned():
 assert os.path.isdir(backup) and not os.path.islink(backup)
 assert os.path.isfile(backup+'/owner') and not os.path.islink(backup+'/owner')
 with open(backup+'/owner') as f:assert f.read()=='momso-upgrade-v1'
 assert os.path.isfile(backup+'/momso-server') and os.path.isfile(backup+'/web/.momso-owned')
if action=='begin':
 lifecycle=os.path.join(data,'lifecycle')
 assert not os.path.islink(lifecycle)
 os.makedirs(lifecycle,mode=0o700,exist_ok=True)
 owner=os.stat(data);os.chown(lifecycle,owner.st_uid,owner.st_gid)
 lock=os.open(os.path.join(lifecycle,'lifecycle.lock'),os.O_RDWR|os.O_CREAT|os.O_NOFOLLOW,0o600)
 os.fchown(lock,owner.st_uid,owner.st_gid);fcntl.flock(lock,fcntl.LOCK_EX|fcntl.LOCK_NB)
 if os.path.exists(os.path.join(lifecycle,'idle-stop.pending')):raise RuntimeError('lifecycle_busy')
 if os.path.exists(backup):raise RuntimeError('recovery_required')
 else:
  assert os.path.isfile(server) and not os.path.islink(server) and os.path.isfile(web+'/.momso-owned') and not os.path.islink(web)
  def optional(path,method='GET'):
   try:return call(path,method)
   except urllib.error.HTTPError as error:
    if error.code in (404,405):return None
    raise
  state=optional('/activity');mode='none' if state is None else 'legacy'
  if state is not None and state.get('maintenance') is True:raise RuntimeError('lifecycle_busy')
  if state is not None and state.get('maintenance') is False:
   result=optional('/maintenance','POST')
   if result is not None:
    if result.get('maintenance') is not True:raise RuntimeError('maintenance_unconfirmed')
    mode='gated'
  deadline=time.monotonic()+120
  while state is not None:
   state=call('/activity')
   if mode=='gated': idle=state.get('quiescent') is True
   else:
    if not isinstance(state.get('busy'),bool):raise RuntimeError('activity_invalid')
    idle=not state['busy'] and not any(state.get(k,0) for k in ('activeTurns','activeWrites','pendingApprovals','pendingInputs','admittedRequests','githubLogins','providerLogin'))
   if idle:break
   if time.monotonic()>=deadline:
    if mode=='gated':call('/maintenance/release','POST')
    raise RuntimeError('server_busy')
   time.sleep(0.25)
  stage=tempfile.mkdtemp(prefix='.momso-backup-',dir=root)
  try:
   subprocess.run(['systemctl','stop','momso-server'],check=True)
   os.chmod(stage,0o700);shutil.copy2(server,stage+'/momso-server');shutil.copytree(web,stage+'/web',symlinks=True)
   snap=DataSnapshots(data,stage+'/snapshots').snapshot()
   with open(stage+'/snapshot-name','w') as f:f.write(os.path.basename(snap))
   if os.path.exists(env):shutil.copy2(env,stage+'/server.env')
   if active:shutil.copy2(os.path.dirname(marker)+'/installed.json',stage+'/installed.json')
   with open(stage+'/mode','w') as f:f.write(mode)
   with open(stage+'/owner','w') as f:f.write('momso-upgrade-v1')
   os.rename(stage,backup)
   if mode=='gated':subprocess.run(['systemctl','start','momso-server'],check=True)
  except Exception:
   if not os.path.exists(backup):
    subprocess.run(['systemctl','start','momso-server'],check=False)
    if mode=='gated':
     try:call('/maintenance/release','POST')
     except Exception:pass
   raise
  finally:
   if os.path.isdir(stage):shutil.rmtree(stage)
def restore_checkpoint():
 global restored_pair
 owned();subprocess.run(['systemctl','stop','momso-server'],check=True)
 managed={};exec(compile(base64.b64decode(MANAGED_CODE),'momso_managed_files.py','exec'),managed)
 errors=[]
 def rollback_managed(path,manifest,journal):
  try:managed['rollback'](path,manifest,journal,INSTALL_OWNER)
  except Exception:errors.append('helper_restore_failed')
 for group,path in (('overrides','/etc/systemd/system/momso-server.service.d'),('units','/etc/systemd/system'),('helper','/opt/momso')):
  rollback_managed(path,'/etc/momso-private/managed-'+group+'.json',backup+'/managed-'+group+'-journal.json')
 rollback_managed('/opt/momso','/etc/momso-private/managed-files.json',backup+'/managed-files.json')
 name=open(backup+'/snapshot-name').read().strip()
 DataSnapshots(data,backup+'/snapshots').restore(backup+'/snapshots/'+name)
 tmp=root+'/.momso-server-restore';shutil.copy2(backup+'/momso-server',tmp);os.replace(tmp,server)
 restored_pair=True
 staged=tempfile.mkdtemp(prefix='.momso-web-restore-',dir=root);os.rmdir(staged)
 try:
  shutil.copytree(backup+'/web',staged,symlinks=True)
  if os.path.exists(web):assert os.path.isfile(web+'/.momso-owned') and not os.path.islink(web);shutil.rmtree(web)
  os.rename(staged,web)
 finally:
  if os.path.isdir(staged):shutil.rmtree(staged)
 if os.path.isfile(backup+'/server.env'):shutil.copy2(backup+'/server.env',env)
 if os.path.isfile(backup+'/installed.json'):shutil.copy2(backup+'/installed.json',os.path.dirname(marker)+'/installed.json')
 subprocess.run(['systemctl','reset-failed','momso-server'],check=False)
 subprocess.run(['systemctl','start','momso-server'],check=True)
 if errors:raise RuntimeError('rollback_failed')
if action=='restore':
 restored_pair=False
 def identity(path):
  try:
   stat=os.stat(path,follow_symlinks=False)
   return (stat.st_dev,stat.st_ino)
  except OSError:return None
 original_data=identity(data);original_server=identity(server)
 try:restore_checkpoint()
 except Exception:
  # Never start an old executable against data whose restoration is uncertain.
  original_pair=(original_data is not None and original_server is not None and identity(data)==original_data and identity(server)==original_server)
  if restored_pair or original_pair:
   subprocess.run(['systemctl','reset-failed','momso-server'],check=False)
   subprocess.run(['systemctl','start','momso-server'],check=False)
  raise
elif action=='release':
 owned()
 if open(backup+'/mode').read()=='gated':call('/maintenance/release','POST')
elif action=='finish':
 owned()
 if open(backup+'/mode').read()=='gated':call('/maintenance/release','POST')
 shutil.rmtree(backup)
'''.replace("ACTION", repr(action)).replace("SNAPSHOT_CODE", repr(snapshot_code)).replace("MANAGED_CODE", repr(managed_code)).replace("INSTALL_OWNER", repr(self.install_id))
        encoded = base64.b64encode(code.encode()).decode()
        wrapper = "import base64,subprocess,sys; r=subprocess.run(['sudo','python3','-c',base64.b64decode(" + repr(encoded) + ").decode()],capture_output=True); print('momso_server_busy' if r.returncode and b'server_busy' in r.stderr else 'momso_restore_failed' if r.returncode and " + repr(action == 'restore') + " else 'momso_checkpoint_failed' if r.returncode else 'momso_checkpoint_ok')"
        result = self.ssh(wrapper).strip()
        if result == b"momso_server_busy":
            raise InstallError("지금 쓰는 중이라 올리지 못했습니다. 잠시 뒤 다시 시도해 주세요.")
        if result == b"momso_restore_failed":
            raise InstallError("복구 중 오류가 발생했습니다. 복구 사본을 보존했습니다.", gateway_failure={'code': 'rollback_failed', 'rolledBack': False, 'retryable': False})
        if result != b"momso_checkpoint_ok":
            raise InstallError("올리기 자료 처리에 실패했습니다. 복구 상태를 확인해 주세요.")

    def upgrade_server(self, artifact: Path, digest: str):
        data = artifact.read_bytes()
        if hashlib.sha256(data).hexdigest() != digest:
            raise InstallError("설치 파일의 SHA-256이 일치하지 않습니다.")
        code = r'''import hashlib,os,subprocess,sys,tempfile
data=sys.stdin.buffer.read()
assert hashlib.sha256(data).hexdigest()==DIGEST
root='/opt/momso';marker=root+'/releases/.momso-update-active';current=root+'/releases/current';override='/etc/systemd/system/momso-server.service.d/momso-update.conf'
active=os.path.exists(marker) or (os.path.isfile(override) and 'ExecStart=/opt/momso/releases/current/momso-server' in open(override).read())
if active:
 assert (not os.path.exists(marker) or (not os.path.islink(marker) and open(marker).read().strip()=='momso-update-v1')) and os.path.islink(current)
 target=os.path.realpath(current);assert os.path.commonpath((target,root+'/releases'))==root+'/releases' and target!=root+'/releases'
 root=target
dest=root+'/momso-server'
assert os.path.isfile(dest) and not os.path.islink(dest)
fd,tmp=tempfile.mkstemp(prefix='.momso-upgrade-',dir=root)
try:
 with os.fdopen(fd,'wb') as f:f.write(data);f.flush();os.fsync(f.fileno())
 os.chmod(tmp,0o755);os.replace(tmp,dest)
 subprocess.run(['systemctl','reset-failed','momso-server'],check=False)
 subprocess.run(['systemctl','restart','momso-server'],check=True)
finally:
 if os.path.exists(tmp):os.unlink(tmp)
'''.replace("DIGEST", repr(digest))
        encoded = base64.b64encode(code.encode()).decode()
        wrapper = "import base64,subprocess,sys; subprocess.run(['sudo','python3','-c',base64.b64decode(" + repr(encoded) + ").decode()],input=sys.stdin.buffer.read(),check=True)"
        self.ssh(wrapper, data)

    def upgrade_sequence(self, sequence, *, commit=False):
        if type(sequence) is not int or sequence < 1:
            raise InstallError("배포 순번을 확인하지 못했습니다.")
        code = r'''import json,os,tempfile
root='/opt/momso/releases';marker=root+'/.momso-update-active';path=root+'/installed.json';override='/etc/systemd/system/momso-server.service.d/momso-update.conf'
active=os.path.exists(marker) or (os.path.isfile(override) and 'ExecStart=/opt/momso/releases/current/momso-server' in open(override).read())
if active:
 assert not os.path.exists(marker) or (not os.path.islink(marker) and open(marker).read().strip()=='momso-update-v1')
 with open(path) as f:previous=json.load(f)['sequence']
 assert type(previous) is int and DESIRED>=previous
 if COMMIT:
  fd,tmp=tempfile.mkstemp(prefix='.sequence-',dir=root)
  try:
   with os.fdopen(fd,'w') as f:json.dump({'sequence':DESIRED},f);f.flush();os.fsync(f.fileno())
   os.replace(tmp,path)
  finally:
   if os.path.exists(tmp):os.unlink(tmp)
'''.replace('DESIRED', repr(sequence)).replace('COMMIT', repr(commit))
        encoded = base64.b64encode(code.encode()).decode()
        wrapper = "import base64,subprocess; subprocess.run(['sudo','python3','-c',base64.b64decode(" + repr(encoded) + ").decode()],check=True)"
        self.ssh(wrapper)

    def install_web(self, archive: Path, digest: str):
        data = archive.read_bytes()
        if hashlib.sha256(data).hexdigest() != digest:
            raise InstallError("화면 파일의 SHA-256이 일치하지 않습니다.")
        code = r'''import hashlib,io,os,pathlib,shutil,subprocess,sys,tarfile,tempfile
data=sys.stdin.buffer.read()
assert hashlib.sha256(data).hexdigest()==DIGEST
root='/opt/momso';marker=root+'/releases/.momso-update-active';current=root+'/releases/current';override='/etc/systemd/system/momso-server.service.d/momso-update.conf'
active=os.path.exists(marker) or (os.path.isfile(override) and 'ExecStart=/opt/momso/releases/current/momso-server' in open(override).read())
if active:
 assert (not os.path.exists(marker) or (not os.path.islink(marker) and open(marker).read().strip()=='momso-update-v1')) and os.path.islink(current)
 target=os.path.realpath(current);assert os.path.commonpath((target,root+'/releases'))==root+'/releases' and target!=root+'/releases'
 root=target
dest=root+'/web'; old=root+'/web.old'
if os.path.exists(dest): assert os.path.isfile(dest+'/.momso-owned')
if os.path.exists(old): assert os.path.isfile(old+'/.momso-owned');shutil.rmtree(old)
tmp=tempfile.mkdtemp(prefix='.web-',dir=root)
try:
 with tarfile.open(fileobj=io.BytesIO(data),mode='r:gz') as tar:
  members=tar.getmembers()
  safe=[]
  for m in members:
   parts=pathlib.PurePosixPath(m.name).parts
   if not parts and m.isdir():continue
   assert parts and not m.name.startswith('/') and '..' not in parts and (m.isdir() or m.isfile())
   safe.append(m)
  tar.extractall(tmp,members=safe,**({'filter':'data'} if hasattr(tarfile,'data_filter') else {}))
 assert os.path.isfile(tmp+'/index.html')
 open(tmp+'/.momso-owned','w').write('momso')
 os.chmod(tmp,0o755)
 for path,dirs,files in os.walk(tmp):
  os.chmod(path,0o755)
  for name in files: os.chmod(os.path.join(path,name),0o644)
 if os.path.exists(dest): os.replace(dest,old)
 os.replace(tmp,dest)
 if os.path.exists(old): shutil.rmtree(old)
 env='/etc/momso-server.env'
 lines=open(env).read().splitlines()
 if 'MOMSO_SERVER_WEB_DIR='+dest not in lines:
  lines=[x for x in lines if not x.startswith('MOMSO_SERVER_WEB_DIR=')]
  lines.append('MOMSO_SERVER_WEB_DIR='+dest)
  with open(env,'w') as f:f.write('\n'.join(lines)+'\n')
 subprocess.run(['systemctl','restart','momso-server'],check=True)
finally:
 if os.path.isdir(tmp): shutil.rmtree(tmp)
'''.replace("DIGEST", repr(digest))
        encoded = base64.b64encode(code.encode()).decode()
        self.ssh("import base64,subprocess,sys; subprocess.run(['sudo','python3','-c',base64.b64decode(" + repr(encoded) + ").decode()],input=sys.stdin.buffer.read(),check=True)", data)

    def destroy(self):
        # Recheck ownership remotely; local checkpoints alone are not proof.
        specs = [("instances", self.name, [f"--zones={ZONE}"], [f"--zone={ZONE}"], True), ("routers", self.router, [f"--regions={REGION}"], [f"--region={REGION}"], False), ("firewall-rules", self.firewall, [], [], False), ("networks subnets", self.subnet, [f"--regions={REGION}"], [f"--region={REGION}"], False), ("networks", self.network, [], [], False)]
        for index, (resource, name, filters, flags, vm) in enumerate(specs):
            parts = resource.split()
            items = self.run("compute", *parts, "list", *filters, f"--filter=name={name}", parse=True)
            found = next((item for item in items if item.get("name") == name), None)
            if not found:
                continue
            owned = found.get("labels", {}).get("momso-install-id") == self.install_id if vm else found.get("description") == self.marker
            if not owned:
                raise InstallError("다른 자원과 이름이 겹쳐 삭제를 중단했습니다.")
            if vm:
                instance = self.run("compute", "instances", "describe", self.name, f"--zone={ZONE}", parse=True)
                disks = instance.get("disks", [])
                if instance.get("labels", {}).get("momso-install-id") != self.install_id or len(disks) != 1 or not disks[0].get("boot") or not disks[0].get("source", "").endswith("/disks/" + self.name):
                    raise InstallError("VM 디스크 구성이 달라 삭제하지 않았습니다.")
            if resource == "routers":
                router = self.run("compute", "routers", "describe", self.router, f"--region={REGION}", parse=True)
                nats = router.get("nats", [])
                if any(nat.get("name") != self.nat for nat in nats):
                    raise InstallError("라우터에 다른 NAT가 있어 삭제하지 않았습니다.")
                if nats:
                    self.run("compute", "routers", "nats", "delete", self.nat, f"--router={self.router}", f"--region={REGION}")
            try:
                self.run("compute", *parts, "delete", name, *flags)
            except InstallError as error:
                if resource not in ("networks subnets", "networks"):
                    raise
                remaining = []
                for kind, candidate, list_flags, _, _ in specs[index:]:
                    try:
                        listed = self.run("compute", *kind.split(), "list", *list_flags, f"--filter=name={candidate}", parse=True)
                        if any(item.get("name") == candidate for item in listed):
                            remaining.append(kind + ":" + candidate)
                    except InstallError:
                        remaining.append(kind + ":" + candidate + " (확인 필요)")
                raise PendingNetworkCleanup(remaining) from error


def deploy(provider, artifact: Path, digest: str, state_path: Path, state: dict):
    if not artifact.is_file() or not re.fullmatch(r"[0-9a-f]{64}", digest):
        raise InstallError("실행 파일과 SHA-256을 확인하세요.")
    if hashlib.sha256(artifact.read_bytes()).hexdigest() != digest:
        raise InstallError("실행 파일의 SHA-256이 일치하지 않습니다.")
    for name, action in [("network", provider.prepare_network), ("vm", provider.prepare_vm), ("install", lambda: provider.install(artifact, digest)), ("health", provider.health)]:
        try:
            action()
        except Exception as error:
            state["steps"][name] = "failed"
            save_state(state_path, state)
            if isinstance(error, InstallError):
                raise
            raise InstallError(f"{name} 단계가 실패했습니다. 원인을 확인한 뒤 다시 실행하세요.") from error
        state["steps"][name] = "ok"
        save_state(state_path, state)


def upgrade_anywhere(provider, state, artifact, digest, web, web_digest, client_id, owner_sub, login_origin, gateway_image, signed_sequence=None):
    import momso_anywhere_install
    from momso_anywhere_public_assets import from_web_archive

    if not re.fullmatch(r"[a-zA-Z0-9._/-]+@sha256:[a-f0-9]{64}", gateway_image or ""):
        raise InstallError("입구 이미지와 sha256 지문을 지정하세요.", gateway_failure=momso_anywhere_install.gateway_failure('gateway_image_invalid'))

    configured = state["anywhere"]
    if any(configured.get(key) != value for key, value in (("client_id", client_id), ("owner_sub", owner_sub), ("login_origin", login_origin))):
        raise InstallError("기존 설치의 Google 로그인 정보가 다릅니다.")
    if not artifact.is_file() or not re.fullmatch(r"[0-9a-f]{64}", digest) or hashlib.sha256(artifact.read_bytes()).hexdigest() != digest:
        raise InstallError("설치 파일의 SHA-256이 일치하지 않습니다.")
    if not web.is_file() or not re.fullmatch(r"[0-9a-f]{64}", web_digest) or hashlib.sha256(web.read_bytes()).hexdigest() != web_digest:
        raise InstallError("화면 묶음의 SHA-256이 일치하지 않습니다.")
    from_web_archive(web)
    vm = provider.run("compute", "instances", "describe", provider.name, f"--zone={ZONE}", parse=True)
    interfaces = vm.get("networkInterfaces", [])
    if vm.get("labels", {}).get("momso-install-id") != provider.install_id or not interfaces or any(item.get("accessConfigs") or item.get("ipv6AccessConfigs") or not item.get("network", "").endswith("/" + provider.network) for item in interfaces):
        raise InstallError("기존 VM의 소유권이나 사설 네트워크를 확인하지 못했습니다.")
    try:
        account, role, service = momso_anywhere_install.GoogleAnywhere(provider)._objects()
    except InstallError as error:
        raise InstallError('기존 입구의 소유권을 확인하지 못했습니다.', gateway_failure=momso_anywhere_install.gateway_failure('gateway_ownership')) from error
    if not account or not role or not service:
        raise InstallError("기존 입구의 소유권을 확인하지 못했습니다.", gateway_failure=momso_anywhere_install.gateway_failure('gateway_ownership'))
    current = provider.run("run", "services", "describe", "momso-any-" + provider.install_id, f"--region={REGION}", parse=True)
    if current.get("status", {}).get("url") != configured.get("url"):
        raise InstallError("기존 입구 주소가 설치 기록과 달라 변경하지 않았습니다.", gateway_failure=momso_anywhere_install.gateway_failure('gateway_address_invalid'))
    if vm.get("status") in ("TERMINATED", "SUSPENDED"):
        provider.run("compute", "instances", "start", provider.name, f"--zone={ZONE}")
    elif vm.get("status") != "RUNNING":
        raise InstallError("기존 VM이 준비 중입니다. 잠시 뒤 다시 해 주세요.")
    try:
        if signed_sequence is not None:
            provider.upgrade_sequence(signed_sequence)
        provider.upgrade_checkpoint("begin")
        try:
            provider.upgrade_server(artifact, digest)
            provider.install_web(web, web_digest)
            provider.health()
            import momso_anywhere_vm_services
            momso_anywhere_vm_services.install(provider, allow_upgrade_gate=True)
            momso_anywhere_install.GoogleAnywhere(provider).upgrade_gateway(configured["url"], gateway_image)
            if signed_sequence is not None:
                provider.upgrade_sequence(signed_sequence, commit=True)
        except (InstallError, OSError, subprocess.SubprocessError) as error:
            restored = False
            gateway_detail = getattr(error, 'gateway_failure', None)
            unsafe_detail = {**gateway_detail, 'retryable': False} if gateway_detail else None
            try:
                provider.upgrade_checkpoint("restore")
                restored = True
                provider.health()
            except (InstallError, OSError, subprocess.SubprocessError) as rollback_error:
                if restored:
                    try:
                        provider.upgrade_checkpoint("release")
                    except (InstallError, OSError, subprocess.SubprocessError) as release_error:
                        raise InstallError("이전 판을 복구했지만 접수 중단을 해제하지 못했습니다. 복구 사본을 보존했습니다.", gateway_failure=unsafe_detail) from release_error
                rollback_detail = getattr(rollback_error, 'gateway_failure', None) or {'code': 'rollback_failed', 'rolledBack': False, 'retryable': False}
                raise InstallError("새 판 확인에 실패했고 이전 판의 복구 상태도 확인하지 못했습니다. 복구 사본을 보존했습니다.", gateway_failure=rollback_detail) from rollback_error
            try:
                provider.upgrade_checkpoint("finish")
            except (InstallError, OSError, subprocess.SubprocessError) as finish_error:
                raise InstallError('이전 판 복구 뒤 정리를 확인하지 못했습니다.', gateway_failure=unsafe_detail) from finish_error
            conflicts = getattr(error, 'managed_conflicts', [])
            suffix = ''.join(f"\n관리 파일 충돌: {name}: {reason}" for name, reason in conflicts)
            raise InstallError("새 판 확인에 실패해 이전 server와 화면으로 되돌렸습니다." + suffix, managed_conflict=conflicts[0] if conflicts else None, managed_conflicts=conflicts, gateway_failure=getattr(error, 'gateway_failure', None)) from error
        provider.upgrade_checkpoint("finish")
        return configured["url"]
    finally:
        previous_error = sys.exc_info()[1]
        try:
            provider.ensure_private_proxy()
        except (InstallError, OSError, subprocess.SubprocessError) as proxy_error:
            detail = getattr(previous_error, 'gateway_failure', None)
            if detail:
                raise InstallError('입구 전환 실패 뒤 프록시 상태를 확인하지 못했습니다.', gateway_failure={**detail, 'retryable': False}) from proxy_error
            raise


# Files distributed with the installer, checked before any customer resources change.
INSTALLER_FILES = (
    "apps/alpha/desktop/src-tauri/momso-server-update/data_snapshots.py",
    "apps/alpha/desktop/src-tauri/momso-server-update/update.py",
    "apps/alpha/desktop/src-tauri/momso-server-update/release_manifest.py",
    "apps/alpha/desktop/src-tauri/momso-server-update/server_admission.py",
    "apps/alpha/desktop/src-tauri/momso-server-update/systemd_host.py",
    "apps/alpha/desktop/src-tauri/momso-server-update/cloud_run_gateway.py",
    "apps/alpha/desktop/src-tauri/momso-server-update/github_releases.py",
    "apps/alpha/desktop/src-tauri/momso-server-update/progress_status.py",
    "apps/alpha/desktop/src-tauri/momso-server-update/layout_migration.py",
    "apps/alpha/desktop/src-tauri/momso-server-update/install_worker.py",
    "apps/alpha/desktop/src-tauri/momso-server-update/worker.py",
    "apps/alpha/desktop/src-tauri/momso-server-update/managed_helpers.py",
    "apps/alpha/desktop/src-tauri/momso-server-update/units/momso-idle-stop.service",
    "apps/alpha/desktop/src-tauri/momso-server-update/units/vault.conf",
    "apps/alpha/desktop/src-tauri/momso-server-update/ensure_gcloud.py",
    "scripts/momso_anywhere_update_install.py",
    "scripts/momso_managed_files.py",
    "scripts/momso_managed_history.py",
    "scripts/momso_managed_history.json",
    "apps/alpha/desktop/src-tauri/momso-vault-provision/provision.py",
    "apps/alpha/desktop/src-tauri/momso-idle-stop/idle_stop.py",
)


def validate_installer_files(root=None):
    root = Path(root) if root is not None else Path(__file__).resolve().parent.parent
    if any(not (root / name).is_file() for name in INSTALLER_FILES):
        raise InstallError("설치에 필요한 파일이 빠졌습니다. 전체 설치기 묶음을 다시 받아 주세요.")


def main(argv=None):
    parser = argparse.ArgumentParser(description="momso server 설치 및 제거")
    parser.add_argument("action", choices=["install", "mobile", "anywhere", "destroy", "delete-preview", "project-exclusivity"])
    parser.add_argument("--account", required=True)
    parser.add_argument("--project", required=True)
    parser.add_argument("--state", type=Path, required=True)
    parser.add_argument('--discard-data', action='store_true', help='백업 최신성 확인을 건너뛰고 VM 자료를 버립니다')
    parser.add_argument("--artifact", type=Path)
    parser.add_argument("--sha256")
    parser.add_argument("--cf-account")
    parser.add_argument("--cf-zone")
    parser.add_argument("--hostname")
    parser.add_argument("--email")
    parser.add_argument("--wake-user", help="깨우기 서비스에 로그인할 Google 계정 이메일")
    parser.add_argument("--web-archive", type=Path)
    parser.add_argument("--web-sha256")
    parser.add_argument("--google-client-id")
    parser.add_argument("--owner-sub")
    parser.add_argument("--login-origin", help="소개 사이트와 별개인 공통 로그인 HTTPS 출처(경로 제외, 시험용 run.app 가능)")
    parser.add_argument("--gateway-image", help="배포할 입구 컨테이너 이미지의 sha256 지문")
    parser.add_argument("--release-manifest-url", help="서명된 배포 목록 HTTPS 주소 (.sig가 같은 위치에 필요)")
    parser.add_argument("--release-public-key", type=Path, default=Path(__file__).with_name('momso_release_public_key.pem'), help="설치기에 고정한 Ed25519 공개키")
    parser.add_argument("--release-channel", choices=['production', 'development'], default='production')
    args = parser.parse_args(argv)
    state = None
    try:
        if args.action == "anywhere":
            validate_installer_files()
        if not re.fullmatch(r"[A-Za-z0-9._%+@-]+", args.account) or not re.fullmatch(r"[a-z][a-z0-9-]{4,28}[a-z0-9]", args.project):
            raise InstallError("계정 또는 프로젝트 인자가 올바르지 않습니다.")
        if args.action in ("destroy", "delete-preview", "project-exclusivity") and not args.state.is_file():
            raise InstallError("설치 기록이 없어 자원을 삭제하지 않았습니다.")
        if args.action in ("install", "mobile", "anywhere") and (args.artifact is None or args.sha256 is None):
            raise InstallError("설치 파일과 SHA-256을 지정하세요.")
        if args.action == "anywhere" and (not args.web_archive or not args.web_sha256 or not re.fullmatch(r"[0-9a-fA-F]{64}", args.web_sha256) or not args.google_client_id or not args.owner_sub or not args.login_origin):
            raise InstallError("화면 묶음과 SHA-256, Google 로그인 앱 ID와 소유자 번호가 필요합니다.")
        if args.action == "anywhere":
            if not args.release_manifest_url or not args.release_public_key.is_file():
                raise InstallError("서명된 배포 목록 주소와 고정된 공개키가 필요합니다.")
            import momso_release_manifest
            import momso_release_keys
            try:
                momso_release_keys.validate(args.release_channel, args.release_public_key)
                signed = momso_release_manifest.release(args.release_manifest_url, args.release_public_key, channel=args.release_channel)
                if args.gateway_image is not None and args.gateway_image != signed['gatewayImage']:
                    raise momso_release_manifest.ManifestError('artifact_mismatch')
                bundle = momso_release_manifest.download(signed['bundleUrl'], 536870912)
                momso_release_manifest.verify_inputs(bundle, signed['bundleSha256'], args.artifact, args.web_archive)
                args.gateway_image = signed['gatewayImage']
            except (momso_release_manifest.ManifestError, OSError, ValueError, KeyError):
                raise InstallError("배포 목록 서명 또는 서버·화면·입구 지문을 확인하지 못했습니다.") from None
        if args.action == "anywhere" and (not args.web_archive.is_file() or hashlib.sha256(args.web_archive.read_bytes()).hexdigest() != args.web_sha256.lower() or not re.fullmatch(r"[A-Za-z0-9._-]+\.apps\.googleusercontent\.com", args.google_client_id) or not re.fullmatch(r"[0-9]{1,40}", args.owner_sub) or not re.fullmatch(r"https://[a-z0-9][a-z0-9.-]*", args.login_origin)):
            raise InstallError("화면 묶음 또는 Google 로그인 정보가 올바르지 않습니다.")
        if args.action == "anywhere":
            import momso_anywhere_public_assets
            try:
                public_assets = momso_anywhere_public_assets.from_web_archive(args.web_archive)
            except (ValueError, OSError, tarfile.TarError) as error:
                raise InstallError("홈 화면용 아이콘과 매니페스트를 확인하지 못했습니다.") from error
        if args.action == "mobile" and (not all((args.cf_account, args.cf_zone, args.hostname, args.email)) or not os.environ.get("MOMSO_CLOUDFLARE_API_TOKEN")):
            raise InstallError("Cloudflare 계정·영역·주소·이메일과 API 권한이 필요합니다.")
        state = initialize_state(args.state, args.account, args.project)
        if args.action == 'project-exclusivity':
            from momso_anywhere_project_exclusivity import inspect_project
            provider = GoogleProvider(args.account, args.project, state['id'], args.state.parent / 'unused-ssh-key')
            print(json.dumps(inspect_project(provider), ensure_ascii=False))
            return 0
        if args.action == 'delete-preview':
            from momso_anywhere_deletion_preview import preview
            provider = GoogleProvider(args.account, args.project, state['id'], args.state.parent / 'unused-ssh-key')
            print(json.dumps(preview(provider), ensure_ascii=False))
            return 0
        if args.action == 'anywhere':
            previous_channel = state.get('anywhere', {}).get('release_channel')
            if previous_channel and previous_channel != args.release_channel:
                raise InstallError('기존 설치의 배포 구분이 달라 변경하지 않았습니다.')
            state.setdefault('anywhere', {'client_id': args.google_client_id, 'owner_sub': args.owner_sub, 'login_origin': args.login_origin})['release_channel'] = args.release_channel
            save_state(args.state, state)
        directory = key_directory(args.state, state["id"], args.action != "destroy")
        key_file = directory / "ssh"
        provider = GoogleProvider(args.account, args.project, state["id"], key_file)
        if args.action == "destroy":
          def destroy_resources():
            if state.get("anywhere") and not state["anywhere"].get("destroyed"):
                import momso_anywhere_install
                momso_anywhere_install.GoogleAnywhere(provider).destroy()
                state["anywhere"]["destroyed"] = True
                save_state(args.state, state)
            if state.get("wake") and not state["wake"].get("destroyed"):
                import momso_wake_install
                momso_wake_install.GoogleWake(provider).destroy()
                state["wake"]["destroyed"] = True
                save_state(args.state, state)
            if state.get("mobile") and not state["mobile"].get("destroyed"):
                if not all((args.cf_account, args.cf_zone, args.hostname, args.email)) or not os.environ.get("MOMSO_CLOUDFLARE_API_TOKEN"):
                    raise InstallError("Cloudflare 자원을 지우려면 원래 계정·영역·주소·이메일과 API 권한이 필요합니다.")
                import momso_mobile_install
                cf = momso_mobile_install.Cloudflare(os.environ["MOMSO_CLOUDFLARE_API_TOKEN"], args.cf_account, args.cf_zone)
                momso_mobile_install.destroy_owned(cf, args.state, state, args.hostname.lower(), args.email, provider.destroy)
            else:
                try:
                    provider.destroy()
                except PendingNetworkCleanup as error:
                    state["steps"]["destroy"] = "pending"
                    state["destroy_pending"] = error.resources
                    save_state(args.state, state)
                    raise
          if state.get('anywhere') and not args.discard_data:
              from momso_anywhere_pc_pause import guarded_destroy
              guarded_destroy(provider, destroy_resources)
          else:
              destroy_resources()
          cleanup_keys(directory, state["id"])
          state.pop("destroy_pending", None)
          from momso_anywhere_pc_inventory import inspect_destroyed
          inventory = inspect_destroyed(provider) if state.get('anywhere') else {}
          if state.get('anywhere') and args.discard_data:
              from momso_anywhere_pc_discard import discard_backup
              from momso_anywhere_delete_result import deletion_result
              deletion = discard_backup(provider, state.get('backup_discard'))
              deletion.setdefault('resources', {}).update(inventory)
              for name in ('vm', 'disk', 'gateway', 'network', 'secrets', 'iam', 'iam_bindings'):
                  deletion.setdefault('resources', {}).setdefault(name, {'status': 'unverified', 'reason': 'not_inventoried'})
              deletion = deletion_result(deletion)
              state['backup_discard'] = deletion
              done = deletion['complete']
              state['steps']['destroy'] = 'ok' if done else 'pending'
              save_state(args.state, state)
              print(json.dumps(deletion, ensure_ascii=False))
              return 0 if done else 1
          state["steps"]["destroy"] = "ok"
          save_state(args.state, state)
          if state.get('anywhere'):
                from momso_anywhere_delete_result import deletion_result
                mode = 'discard' if args.discard_data else 'pause'
                status = 'backup_retained_unverified' if args.discard_data else 'backup_current_verified'
                deletion = deletion_result({'mode': mode, 'status': status, 'resources': inventory, 'retained': [
                    {'resource': 'secret_vault', 'status': 'unverified', 'reason': 'not_inspected'},
                    {'resource': 'backup_bucket', 'status': 'unverified', 'reason': 'not_inspected'},
                    {'resource': 'backup_kms', 'status': 'unverified', 'reason': 'not_inspected'},
                ], 'billingZero': None})
                state['destroy_result'] = deletion
                save_state(args.state, state)
                print(json.dumps(deletion, ensure_ascii=False))
                return 0 if deletion['complete'] else 1
        else:
            existing = state.get("anywhere", {})
            if args.action == "anywhere" and existing.get("url") and not existing.get("destroyed"):
                url = upgrade_anywhere(provider, state, args.artifact, args.sha256.lower(), args.web_archive, args.web_sha256.lower(), args.google_client_id, args.owner_sub, args.login_origin, args.gateway_image, signed['sequence'])
                print("접속 주소: " + url)
            else:
                deploy(provider, args.artifact, args.sha256.lower(), args.state, state)
            if args.action == "mobile":
                import momso_mobile_install
                cf = momso_mobile_install.Cloudflare(os.environ["MOMSO_CLOUDFLARE_API_TOKEN"], args.cf_account, args.cf_zone)
                momso_mobile_install.connect(provider, cf, args.state, state, args.hostname.lower(), args.email)
                if args.wake_user:
                    import momso_wake_install
                    url = momso_wake_install.install(momso_wake_install.GoogleWake(provider), args.state, state, args.hostname.lower(), args.wake_user)
                    print("깨우기 주소: " + url)
            elif args.action == "anywhere" and not (existing.get("url") and not existing.get("destroyed")):
                provider.install_web(args.web_archive, args.web_sha256.lower())
                import momso_anywhere_install
                url = momso_anywhere_install.install(provider, args.state, state, args.google_client_id, args.owner_sub, args.login_origin, args.gateway_image)
                print("접속 주소: " + url)
            if args.action == "anywhere":
                import momso_anywhere_update_install
                momso_anywhere_update_install.grant_gateway_update(provider)
                momso_anywhere_update_install.install(provider, args.release_channel, args.release_public_key, args.release_manifest_url)
        print("완료: " + args.action)
        return 0
    except (InstallError, OSError, subprocess.SubprocessError, ValueError) as error:
        if isinstance(error, InstallError) and error.gateway_failure:
            print(json.dumps(error.gateway_failure, ensure_ascii=False))
        if args.action == 'destroy':
            from momso_anywhere_delete_result import deletion_result
            code = str(error)
            if not re.fullmatch(r'[a-z][a-z0-9_]{0,79}', code):
                code = 'delete_failed'
            previous = state.get('backup_discard') if isinstance(state, dict) else None
            detail = dict(previous) if isinstance(previous, dict) else {}
            detail['mode'] = 'delete_all' if args.discard_data else 'pause'
            if isinstance(error, PendingNetworkCleanup):
                known = [name for name in error.resources if not name.endswith(' (확인 필요)')]
                unknown = [name.removesuffix(' (확인 필요)') for name in error.resources if name.endswith(' (확인 필요)')]
                detail.update(remaining=known, resources={name: {'status': 'unverified', 'reason': 'inventory_unavailable'} for name in unknown}, retryable=True)
                if not known:
                    detail.update(failed=True, reason='cleanup_incomplete')
            else:
                detail.update(failed=True, reason=code)
            print(json.dumps(deletion_result(detail), ensure_ascii=False))
        print(f"실패: {error}", file=sys.stderr)
        return 1


if __name__ == "__main__":
    sys.exit(main())
