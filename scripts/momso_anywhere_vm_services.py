"""Install customer-VM vault and idle services; never handle their key on the PC."""
import base64
import json
from pathlib import Path
import time

from momso_server_install import InstallError, ZONE

SCOPE = "https://www.googleapis.com/auth/cloud-platform"


def _paths():
    root = Path(__file__).resolve().parent.parent / "apps/alpha/desktop/src-tauri"
    scripts = Path(__file__).resolve().parent
    result = {"vault-provision.py": (root / "momso-vault-provision/provision.py").read_bytes(),
              "idle-stop.py": (root / "momso-idle-stop/idle_stop.py").read_bytes()}
    for name in ("momso_anywhere_backup.py", "momso_anywhere_backup_agent.py", "momso_anywhere_backup_google.py", "momso_anywhere_vault_identity.py"):
        result[name] = (scripts / name).read_bytes()
    return result


def identity(provider):
    p = provider
    name = "mv-" + p.install_id
    email = name + "@" + p.project + ".iam.gserviceaccount.com"
    role = "momsoStop" + p.install_id
    accounts = p.run("iam", "service-accounts", "list", parse=True)
    existing = next((item for item in accounts if item.get("email") == email), None)
    if existing:
        if p.run("iam", "service-accounts", "describe", email, parse=True).get("description") != p.marker:
            raise InstallError("VM 서비스 계정의 소유권이 다릅니다.")
    else:
        p.run("iam", "service-accounts", "create", name, f"--description={p.marker}")
    roles = p.run("iam", "roles", "list", parse=True)
    existing = next((item for item in roles if item.get("name", "").endswith("/roles/" + role)), None)
    if existing:
        value = p.run("iam", "roles", "describe", role, parse=True)
        if value.get("description") != p.marker or value.get("deleted") or set(value.get("includedPermissions", [])) != {"compute.instances.stop"}:
            raise InstallError("VM 종료 권한의 소유권이나 범위가 다릅니다.")
    else:
        p.run("iam", "roles", "create", role, f"--title={role}", f"--description={p.marker}", "--permissions=compute.instances.stop", "--stage=GA")
    for attempt in range(12):
        try:
            p.run("compute", "instances", "add-iam-policy-binding", p.name, f"--zone={ZONE}", f"--member=serviceAccount:{email}", f"--role=projects/{p.project}/roles/{role}")
            break
        except InstallError:
            if attempt == 11: raise
            time.sleep(5)
    return email


def secret(provider, email):
    p = provider
    name = "momso-vault-" + p.install_id
    entries = p.run("secrets", "list", parse=True)
    found = next((v for v in entries if v.get("name", "").endswith("/secrets/" + name)), None)
    if found and found.get("labels", {}).get("momso-install-id") != p.install_id:
        raise InstallError("보관함 열쇠의 소유권이 다릅니다.")
    if not found:
        p.run("secrets", "create", name, "--replication-policy=automatic", f"--labels=momso-install-id={p.install_id}")
    versions = p.run("secrets", "versions", "list", name, parse=True)
    for role in ("roles/secretmanager.secretAccessor", "roles/secretmanager.secretVersionAdder"):
        p.run("secrets", "add-iam-policy-binding", name, f"--member=serviceAccount:{email}", f"--role={role}")
    return name, not versions


def _release_gate(p):
    release = r'''import json,os,sys,urllib.request
marker='/etc/momso-private/s56-identity-gate'
if not os.path.exists(marker):sys.exit(0)
assert os.path.isfile(marker) and open(marker).read().strip()==OWNER
token=next(x.split('=',1)[1] for x in open('/etc/momso-server.env') if x.startswith('MOMSO_SERVER_TOKEN=')).strip()
r=urllib.request.Request('http://127.0.0.1:8765/maintenance/release',headers={'Authorization':'Bearer '+token},method='POST',data=b'')
assert json.load(urllib.request.urlopen(r,timeout=5)).get('maintenance') is False
os.unlink(marker)
'''.replace("OWNER", repr(p.install_id))
    wrapper = "import base64,subprocess; subprocess.run(['sudo','python3','-c',base64.b64decode(" + repr(base64.b64encode(release.encode()).decode()) + ").decode()],check=True)"
    p.ssh(wrapper)



def attach(provider, email):
    p = provider
    vm = p.run("compute", "instances", "describe", p.name, f"--zone={ZONE}", parse=True)
    accounts = vm.get("serviceAccounts", [])
    if accounts:
        if len(accounts) != 1 or accounts[0].get("email") != email or SCOPE not in accounts[0].get("scopes", []):
            raise InstallError("기존 VM 서비스 계정이나 범위가 달라 덮어쓰지 않았습니다.")
    # Keep work paused through service installation and any server restart.
    gate = r'''import fcntl,json,os,pwd,urllib.request
directory='/var/lib/momso/lifecycle'
if os.path.islink(directory):raise RuntimeError('unsafe_lifecycle')
os.makedirs(directory,mode=0o700,exist_ok=True)
a=pwd.getpwnam('momso');os.chown(directory,a.pw_uid,a.pw_gid)
fd=os.open(directory+'/lifecycle.lock',os.O_CREAT|os.O_RDWR|os.O_NOFOLLOW,0o600);os.fchown(fd,a.pw_uid,a.pw_gid);fcntl.flock(fd,fcntl.LOCK_EX|fcntl.LOCK_NB)
p='/etc/momso-server.env'; token=next(x.split('=',1)[1] for x in open(p) if x.startswith('MOMSO_SERVER_TOKEN=')) .strip()
def call(path,method='GET'):
 r=urllib.request.Request('http://127.0.0.1:8765'+path,headers={'Authorization':'Bearer '+token},method=method,data=b'' if method=='POST' else None)
 return json.load(urllib.request.urlopen(r,timeout=5))
marker='/etc/momso-private/s56-identity-gate'
os.makedirs('/etc/momso-private',mode=0o700,exist_ok=True)
if os.path.isfile(marker):
 assert open(marker).read().strip()==OWNER
else:
 assert call('/activity').get('maintenance') is False
 with open(marker,'x') as f:f.write(OWNER);f.flush();os.fsync(f.fileno())
call('/maintenance','POST')
if not call('/activity').get('quiescent'):
 call('/maintenance/release','POST');os.unlink(marker);raise RuntimeError('server_busy')
'''.replace("OWNER", repr(p.install_id))
    wrapper = "import base64,subprocess; subprocess.run(['sudo','python3','-c',base64.b64decode(" + repr(base64.b64encode(gate.encode()).decode()) + ").decode()],check=True)"
    p.ssh(wrapper)
    if not accounts:
        stopped = False
        try:
            p.run("compute", "instances", "stop", p.name, f"--zone={ZONE}")
            stopped = True
            p.run("compute", "instances", "set-service-account", p.name, f"--zone={ZONE}", f"--service-account={email}", f"--scopes={SCOPE}")
        finally:
            if stopped:
                p.run("compute", "instances", "start", p.name, f"--zone={ZONE}")
        p.health()

def install_scripts(provider, name, initial, backup):
    p = provider
    files = {key: base64.b64encode(value).decode("ascii") for key, value in _paths().items()}
    payload = r'''import base64,json,os,shutil,subprocess,sys,tempfile
files=json.load(sys.stdin)
def run(*args):subprocess.run(args,check=True,stdout=subprocess.DEVNULL,stderr=subprocess.DEVNULL)
missing=[]
if not shutil.which('age-keygen'):missing.append('age')
if subprocess.run(['/usr/bin/python3','-c','import cryptography'],stdout=subprocess.DEVNULL,stderr=subprocess.DEVNULL).returncode:missing.append('python3-cryptography')
if missing:
 os.environ['DEBIAN_FRONTEND']='noninteractive';run('apt-get','update','-qq');run('apt-get','install','-y','-qq',*missing)
os.makedirs('/opt/momso',exist_ok=True)
for name,value in files.items():
 path='/opt/momso/'+name; tmp=path+'.new'
 if name.startswith('momso_anywhere_') and os.path.lexists(path):
  if os.path.islink(path) or not os.path.isfile(path) or open(path,'rb').read()!=base64.b64decode(value,validate=True):raise RuntimeError('backup_worker_conflict')
  continue
 if name.startswith('momso_anywhere_'):
  fd,temporary=tempfile.mkstemp(prefix='.momso-backup-worker-',dir='/opt/momso')
  try:
   with os.fdopen(fd,'wb') as f:f.write(base64.b64decode(value,validate=True));f.flush();os.fsync(f.fileno())
   os.chmod(temporary,0o755);os.link(temporary,path)
  finally:os.unlink(temporary)
  continue
 with open(tmp,'wb') as f:f.write(base64.b64decode(value,validate=True));f.flush();os.fsync(f.fileno())
 os.chmod(tmp,0o755);os.replace(tmp,path)
project=PROJECT; secret=SECRET
cmd=['/usr/bin/python3','/opt/momso/vault-provision.py','--project',project,'--secret',secret,'--vault-dir','/var/lib/momso']
if INITIAL:cmd.append('--create-initial')
run(*cmd)
helper="""import os
p='/etc/momso-server.env'
with open(p) as f: token=next(line.split('=',1)[1].strip() for line in f if line.startswith('MOMSO_SERVER_TOKEN='))
assert token and '\\n' not in token
p='/run/momso-vault/server-token';t=p+'.new'
fd=os.open(t,os.O_WRONLY|os.O_CREAT|os.O_EXCL|os.O_NOFOLLOW,0o600)
with os.fdopen(fd,'w') as f:f.write(token)
os.replace(t,p)
"""
open('/opt/momso/idle-token.py','w').write(helper);os.chmod('/opt/momso/idle-token.py',0o755)
run('/usr/bin/python3','/opt/momso/idle-token.py')
base='[Unit]\nDescription=momso vault credential\nAfter=network-online.target\nWants=network-online.target\n[Service]\nType=oneshot\nRestart=on-failure\nRestartSec=10\nExecStart=/usr/bin/python3 /opt/momso/vault-provision.py --project '+project+' --secret '+secret+' --vault-dir /var/lib/momso\nExecStartPost=/usr/bin/python3 /opt/momso/idle-token.py\nRemainAfterExit=yes\n[Install]\nWantedBy=multi-user.target\n'
open('/etc/systemd/system/momso-vault-credential.service','w').write(base)
os.makedirs('/etc/systemd/system/momso-server.service.d',exist_ok=True)
open('/etc/systemd/system/momso-server.service.d/vault.conf','w').write('[Unit]\nRequires=momso-vault-credential.service\nAfter=momso-vault-credential.service\n[Service]\nLoadCredential=momso-vault-identity:/run/momso-vault/momso-vault-identity\n')
conf='/etc/systemd/system/momso-server.service.d/backup.conf'
contents='[Service]\nEnvironment=MOMSO_BACKUP_AGENT=/opt/momso/momso_anywhere_backup_agent.py\nEnvironment=MOMSO_BACKUP_BUCKET='+BACKUP_BUCKET_VALUE+'\nEnvironment=MOMSO_BACKUP_KMS_KEY='+BACKUP_KEY_VALUE+'\n'
if os.path.lexists(conf):
 if os.path.islink(conf) or not os.path.isfile(conf) or open(conf).read()!=contents:raise RuntimeError('backup_configuration_conflict')
else:
 fd=os.open(conf,os.O_CREAT|os.O_EXCL|os.O_WRONLY|os.O_NOFOLLOW,0o644)
 with os.fdopen(fd,'w') as f:f.write(contents);f.flush();os.fsync(f.fileno())
idle='[Unit]\nDescription=momso idle stop\nRequires=momso-server.service momso-vault-credential.service\nAfter=momso-server.service momso-vault-credential.service\n[Service]\nUser=momso\nLoadCredential=server-token:/run/momso-vault/server-token\nExecStart=/usr/bin/python3 /opt/momso/idle-stop.py --token-file ${CREDENTIALS_DIRECTORY}/server-token --state-dir /var/lib/momso/lifecycle\nRestart=on-failure\nRestartSec=60\n[Install]\nWantedBy=multi-user.target\n'
open('/etc/systemd/system/momso-idle-stop.service','w').write(idle)
run('systemctl','daemon-reload');run('systemctl','enable','--now','momso-vault-credential');run('systemctl','restart','momso-server');run('systemctl','enable','--now','momso-idle-stop');run('systemctl','restart','momso-idle-stop')
'''.replace("PROJECT", repr(p.project)).replace("SECRET", repr(name)).replace("INITIAL", repr(initial)).replace("BACKUP_BUCKET_VALUE", repr(backup["bucket"])).replace("BACKUP_KEY_VALUE", repr(backup["key"]))
    wrapper = "import base64,subprocess,sys; subprocess.run(['sudo','python3','-c',base64.b64decode(" + repr(base64.b64encode(payload.encode()).decode()) + ").decode()],input=sys.stdin.buffer.read(),check=True)"
    for attempt in range(12):
        try:
            p.ssh(wrapper, json.dumps(files).encode())
            break
        except InstallError:
            if attempt == 5:
                raise
            time.sleep(5)
    p.health()


def install(provider):
    vm = provider.run("compute", "instances", "describe", provider.name, f"--zone={ZONE}", parse=True)
    if vm.get("labels", {}).get("momso-install-id") != provider.install_id:
        raise InstallError("VM 소유권이 다릅니다.")
    accounts = vm.get("serviceAccounts", [])
    expected = "mv-" + provider.install_id + "@" + provider.project + ".iam.gserviceaccount.com"
    if accounts and (len(accounts) != 1 or accounts[0].get("email") != expected or SCOPE not in accounts[0].get("scopes", [])):
        raise InstallError("기존 VM 서비스 계정이나 범위가 달라 덮어쓰지 않았습니다.")
    provider.run("services", "enable", "secretmanager.googleapis.com", "storage.googleapis.com", "cloudkms.googleapis.com")
    email = identity(provider)
    from momso_anywhere_backup_provision import GoogleBackupProvisioner
    backup = GoogleBackupProvisioner(provider, email).provision()
    name, initial = secret(provider, email)
    try:
        attach(provider, email)
        try:
            install_scripts(provider, name, initial, backup)
        finally:
            _release_gate(provider)
    finally:
        provider.run("secrets", "remove-iam-policy-binding", name, f"--member=serviceAccount:{email}", "--role=roles/secretmanager.secretVersionAdder")
