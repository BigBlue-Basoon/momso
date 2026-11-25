"""Install customer-VM vault and idle services; never handle their key on the PC."""
import base64
import json
from pathlib import Path
import time
import textwrap

from momso_server_install import InstallError, ZONE
from momso_iam_retry import bind_custom_role, reconcile_owned_role
from momso_managed_history import load as load_managed_history
from momso_anywhere_vm_resources import names as vm_resource_names

SCOPE = "https://www.googleapis.com/auth/cloud-platform"

LEGACY_FILES = load_managed_history()['files']


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
    name = vm_resource_names(p)['account']
    email = name + "@" + p.project + ".iam.gserviceaccount.com"
    role = vm_resource_names(p)['roles'][0]
    reconcile_owned_role(p, role, {'compute.instances.stop'})
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
    bind_custom_role(p, "compute", "instances", "add-iam-policy-binding", p.name, f"--zone={ZONE}", f"--member=serviceAccount:{email}", f"--role=projects/{p.project}/roles/{role}")
    return email


def secret(provider, email):
    p = provider
    name = vm_resource_names(p)['vault_secret']
    entries = p.run("secrets", "list", parse=True)
    found = next((v for v in entries if v.get("name", "").endswith("/secrets/" + name)), None)
    if found and found.get("labels", {}).get("momso-install-id") != p.install_id:
        raise InstallError("보관함 열쇠의 소유권이 다릅니다.")
    if not found:
        p.run("secrets", "create", name, "--replication-policy=automatic", f"--labels=momso-install-id={p.install_id}")
    versions = p.run("secrets", "versions", "list", name, parse=True)
    for role in ("roles/secretmanager.secretAccessor", "roles/secretmanager.secretVersionAdder"):
        bind_custom_role(p, "secrets", "add-iam-policy-binding", name, f"--member=serviceAccount:{email}", f"--role={role}")
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



def attach(provider, email, *, allow_upgrade_gate=False):
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
borrowed=False
if ALLOW_UPGRADE:
 backup='/opt/momso/.momso-upgrade-backup'
 assert os.path.isdir(backup) and not os.path.islink(backup)
 for name in ('owner','mode'):
  assert os.path.isfile(backup+'/'+name) and not os.path.islink(backup+'/'+name)
 assert open(backup+'/owner').read()=='momso-upgrade-v1'
 mode=open(backup+'/mode').read()
 assert mode in ('gated','legacy','none')
 borrowed=mode=='gated'
if borrowed:
 assert not os.path.lexists(marker)
 state=call('/activity')
 assert state.get('maintenance') is True and state.get('quiescent') is True
else:
 if os.path.isfile(marker):
  assert not os.path.islink(marker) and open(marker).read().strip()==OWNER
 else:
  assert not os.path.lexists(marker) and call('/activity').get('maintenance') is False
  with open(marker,'x') as f:f.write(OWNER);f.flush();os.fsync(f.fileno())
 call('/maintenance','POST')
 if not call('/activity').get('quiescent'):
  call('/maintenance/release','POST');os.unlink(marker);raise RuntimeError('server_busy')
'''.replace("OWNER", repr(p.install_id)).replace("ALLOW_UPGRADE", repr(allow_upgrade_gate))
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
os.makedirs('/opt/momso',mode=0o755,exist_ok=True)
os.makedirs('/etc/momso-private',mode=0o700,exist_ok=True)
if os.path.islink('/etc/momso-private'):raise RuntimeError('managed_directory_conflict')
managed={};exec(compile(base64.b64decode(MANAGED_MODULE),'momso_managed_files.py','exec'),managed)
manifest='/etc/momso-private/managed-files.json'
checkpoint='/opt/momso/.momso-upgrade-backup'
if os.path.lexists(checkpoint):
 if os.path.islink(checkpoint) or not os.path.isdir(checkpoint) or open(checkpoint+'/owner').read()!='momso-upgrade-v1':raise RuntimeError('managed_checkpoint_conflict')
 journal=checkpoint+'/managed-files.json'
else:journal='/etc/momso-private/managed-files-journal.json'
project=PROJECT; secret=SECRET
helper="""import os
p='/etc/momso-server.env'
with open(p) as f: token=next(line.split('=',1)[1].strip() for line in f if line.startswith('MOMSO_SERVER_TOKEN='))
assert token and '\\n' not in token
p='/run/momso-vault/server-token';t=p+'.new'
fd=os.open(t,os.O_WRONLY|os.O_CREAT|os.O_EXCL|os.O_NOFOLLOW,0o600)
with os.fdopen(fd,'w') as f:f.write(token)
os.replace(t,p)
"""
base='[Unit]\nDescription=momso vault credential\nAfter=network-online.target\nWants=network-online.target\n[Service]\nType=oneshot\nRestart=on-failure\nRestartSec=10\nExecStart=/usr/bin/python3 /opt/momso/vault-provision.py --project '+project+' --secret '+secret+' --vault-dir /var/lib/momso\nExecStartPost=/usr/bin/python3 /opt/momso/idle-token.py\nRemainAfterExit=yes\n[Install]\nWantedBy=multi-user.target\n'
vault='[Unit]\nRequires=momso-vault-credential.service\nAfter=momso-vault-credential.service\n[Service]\nLoadCredential=momso-vault-identity:/run/momso-vault/momso-vault-identity\n'
conf='/etc/systemd/system/momso-server.service.d/backup.conf'
contents='[Service]\nEnvironment=MOMSO_BACKUP_AGENT=/opt/momso/momso_anywhere_backup_agent.py\nEnvironment=MOMSO_BACKUP_BUCKET='+BACKUP_BUCKET_VALUE+'\nEnvironment=MOMSO_BACKUP_KMS_KEY='+BACKUP_KEY_VALUE+'\n'
idle='[Unit]\nDescription=momso idle stop\nRequires=momso-server.service momso-vault-credential.service\nAfter=momso-server.service momso-vault-credential.service\n[Service]\nUser=momso\nLoadCredential=server-token:/run/momso-vault/server-token\nExecStart=/usr/bin/python3 /opt/momso/idle-stop.py --token-file ${CREDENTIALS_DIRECTORY}/server-token --state-dir /var/lib/momso/lifecycle\nRestart=on-failure\nRestartSec=60\n[Install]\nWantedBy=multi-user.target\n'
first=not os.path.lexists(manifest)
raw={name:base64.b64decode(value,validate=True) for name,value in files.items()}
specs=[('/opt/momso',manifest,journal,raw,LEGACY,0o755,()),
 ('/opt/momso','/etc/momso-private/managed-helper.json',(checkpoint if os.path.isdir(checkpoint) else '/etc/momso-private')+'/managed-helper-journal.json',{'idle-token.py':helper.encode()}, {},0o755,('idle-token.py',)),
 ('/etc/systemd/system','/etc/momso-private/managed-units.json',(checkpoint if os.path.isdir(checkpoint) else '/etc/momso-private')+'/managed-units-journal.json',{'momso-vault-credential.service':base.encode(),'momso-idle-stop.service':idle.encode()}, {},0o644,('momso-vault-credential.service','momso-idle-stop.service')),
 ('/etc/systemd/system/momso-server.service.d','/etc/momso-private/managed-overrides.json',(checkpoint if os.path.isdir(checkpoint) else '/etc/momso-private')+'/managed-overrides-journal.json',{'vault.conf':vault.encode(),'backup.conf':contents.encode()}, {},0o644,('vault.conf','backup.conf'))]
issues=[]
for r,m,j,data,legacy,mode,adopt in specs:issues.extend(managed['conflicts'](r,m,OWNER,data,legacy,adopt=adopt,first_migration=first))
if issues:raise RuntimeError('\n'.join('momso-managed-conflict: '+n+': '+why for n,why in issues))
missing=[]
if not shutil.which('age-keygen'):missing.append('age')
if subprocess.run(['/usr/bin/python3','-c','import cryptography'],stdout=subprocess.DEVNULL,stderr=subprocess.DEVNULL).returncode:missing.append('python3-cryptography')
if missing:
 os.environ['DEBIAN_FRONTEND']='noninteractive';run('apt-get','update','-qq');run('apt-get','install','-y','-qq',*missing)
os.makedirs('/etc/systemd/system/momso-server.service.d',mode=0o755,exist_ok=True)
groups=[]
for r,m,j,data,legacy,mode,adopt in specs:
 managed['install'](r,m,j,OWNER,data,legacy,mode,adopt=adopt,first_migration=first)
 groups.append((r,m,j))
cmd=['/usr/bin/python3','/opt/momso/vault-provision.py','--project',project,'--secret',secret,'--vault-dir','/var/lib/momso']
if INITIAL:cmd.append('--create-initial')
run(*cmd)
run('/usr/bin/python3','/opt/momso/idle-token.py')
run('systemctl','daemon-reload');run('systemctl','enable','--now','momso-vault-credential');run('systemctl','restart','momso-server');run('systemctl','enable','--now','momso-idle-stop');run('systemctl','restart','momso-idle-stop')
'''.replace("PROJECT", repr(p.project)).replace("SECRET", repr(name)).replace("INITIAL", repr(initial)).replace("BACKUP_BUCKET_VALUE", repr(backup["bucket"])).replace("BACKUP_KEY_VALUE", repr(backup["key"]))
    # Roll back managed files if any later installation or service step fails.
    start = payload.index("groups=[]")
    before, rest = payload[:start], payload[start:]
    payload = (before + "try:\n" + textwrap.indent(rest, " ")
               + "\nexcept BaseException:\n for r,m,j in reversed(groups):managed['rollback'](r,m,j,OWNER)\n raise\n"
               + "for r,m,j in groups:\n if j.startswith('/etc/'):os.unlink(j)\n")
    module = (Path(__file__).resolve().parent / 'momso_managed_files.py').read_bytes()
    payload = payload.replace('MANAGED_MODULE', repr(base64.b64encode(module).decode('ascii'))).replace('OWNER', repr(p.install_id)).replace('LEGACY', repr(LEGACY_FILES))
    wrapper = "import base64,subprocess,sys; subprocess.run(['sudo','python3','-c',base64.b64decode(" + repr(base64.b64encode(payload.encode()).decode()) + ").decode()],input=sys.stdin.buffer.read(),check=True)"
    for attempt in range(12):
        try:
            p.ssh(wrapper, json.dumps(files).encode())
            break
        except InstallError as error:
            if error.managed_conflict or attempt == 5:
                raise
            time.sleep(5)
    p.health()


def install(provider, *, allow_upgrade_gate=False):
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
        try:
            attach(provider, email, allow_upgrade_gate=allow_upgrade_gate)
            install_scripts(provider, name, initial, backup)
        finally:
            _release_gate(provider)
    finally:
        provider.run("secrets", "remove-iam-policy-binding", name, f"--member=serviceAccount:{email}", "--role=roles/secretmanager.secretVersionAdder")
