"""Send the updater and its public configuration to an owned customer VM."""
import base64
import json
from pathlib import Path
import textwrap
from urllib.parse import urlsplit, urlunsplit

from momso_server_install import InstallError, REGION
from momso_managed_history import load as load_managed_history

_HISTORY = load_managed_history()
LEGACY_WORKER = _HISTORY['worker']
LEGACY_PROXY = _HISTORY['proxy']


def refresh_worker(provider):
    """Replace only managed worker files and restart it; preserve recovery journal and data."""
    source = Path(__file__).resolve().parent.parent / 'apps/alpha/desktop/src-tauri/momso-server-update'
    names = ('update.py', 'release_manifest.py', 'server_admission.py', 'systemd_host.py',
             'data_snapshots.py', 'cloud_run_gateway.py', 'github_releases.py', 'progress_status.py',
             'layout_migration.py', 'install_worker.py', 'worker.py', 'ensure_gcloud.py')
    payload = {name: base64.b64encode((source / name).read_bytes()).decode('ascii') for name in names}
    module = (Path(__file__).resolve().parent / 'momso_managed_files.py').read_bytes()
    code = r'''import base64,json,os,pathlib,subprocess,sys
data=json.load(sys.stdin)
module={};exec(compile(base64.b64decode(MANAGED_MODULE),'momso_managed_files.py','exec'),module)
root=pathlib.Path('/opt/momso-updater')
config=pathlib.Path('/etc/momso-update.json');unit=pathlib.Path('/etc/systemd/system/momso-update.service')
expected='ExecStart=/usr/bin/python3 /opt/momso-updater/worker.py --config /etc/momso-update.json'
if (not root.is_dir() or root.is_symlink() or config.is_symlink() or not config.is_file()
    or unit.is_symlink() or not unit.is_file() or expected not in unit.read_text().splitlines()):raise RuntimeError('worker_not_installed')
manifest='/etc/momso-private/managed-update-worker.json';journal='/etc/momso-private/managed-update-worker-journal.json'
try:
 subprocess.run(['systemctl','stop','momso-update.service'],check=True,timeout=60)
 module['install'](root,manifest,journal,OWNER,{name:base64.b64decode(value,validate=True) for name,value in data.items()},LEGACY,0o644)
 os.unlink(journal)
except BaseException:
 if os.path.exists(journal):module['rollback'](root,manifest,journal,OWNER)
 raise
finally:
 subprocess.run(['systemctl','start','momso-update.service'],check=True,timeout=60)
'''
    code = code.replace('MANAGED_MODULE', repr(base64.b64encode(module).decode('ascii'))).replace('OWNER', repr(provider.install_id)).replace('LEGACY', repr(LEGACY_WORKER))
    wrapper = "import base64,subprocess,sys; subprocess.run(['sudo','python3','-c',base64.b64decode(" + repr(base64.b64encode(code.encode()).decode('ascii')) + ").decode()],input=sys.stdin.buffer.read(),check=True)"
    provider.ssh(wrapper, json.dumps(payload).encode())


def grant_gateway_update(provider):
    """Grant only the customer's VM access to its own gateway and runtime identity."""
    from momso_iam_retry import bind_custom_role
    role = "momsoUpdate" + provider.install_id
    vm = "mv-" + provider.install_id + "@" + provider.project + ".iam.gserviceaccount.com"
    runtime = "ma-" + provider.install_id + "@" + provider.project + ".iam.gserviceaccount.com"
    service = "momso-any-" + provider.install_id
    permissions = {"run.services.get", "run.services.update", "run.revisions.get", "run.revisions.list", "run.operations.get"}
    roles = provider.run("iam", "roles", "list", parse=True)
    existing = next((item for item in roles if item.get("name", "").endswith("/roles/" + role)), None)
    if existing:
        actual = provider.run("iam", "roles", "describe", role, parse=True)
        if actual.get("description") != provider.marker or actual.get("deleted") or set(actual.get("includedPermissions", [])) != permissions:
            raise InstallError("업데이트 권한의 소유권이나 범위가 다릅니다.")
    else:
        provider.run("iam", "roles", "create", role, f"--title={role}", f"--description={provider.marker}",
                     "--permissions=" + ",".join(sorted(permissions)), "--stage=GA")
    bind_custom_role(provider, "run", "services", "add-iam-policy-binding", service, f"--region={REGION}",
                     f"--member=serviceAccount:{vm}", f"--role=projects/{provider.project}/roles/{role}")
    provider.run("iam", "service-accounts", "add-iam-policy-binding", runtime,
                 f"--member=serviceAccount:{vm}", "--role=roles/iam.serviceAccountUser")


def install(provider, channel, public_key, manifest_url):
    if channel not in ("development", "production"):
        raise InstallError("업데이트 채널을 확인하지 못했습니다.")
    parsed = urlsplit(manifest_url)
    if parsed.scheme != "https" or not parsed.hostname or parsed.username or parsed.password or parsed.fragment:
        raise InstallError("업데이트 배포 주소를 확인하지 못했습니다.")
    signature = urlunsplit((parsed.scheme, parsed.netloc, parsed.path + ".sig", parsed.query, ""))
    source = Path(__file__).resolve().parent.parent / "apps/alpha/desktop/src-tauri/momso-server-update"
    names = ("update.py", "release_manifest.py", "server_admission.py", "systemd_host.py",
             "data_snapshots.py", "cloud_run_gateway.py", "github_releases.py", "progress_status.py",
             "layout_migration.py", "install_worker.py", "worker.py", "ensure_gcloud.py")
    config = {"channel": channel, "updateRoot": "/opt/momso/releases", "dataDir": "/var/lib/momso",
              "statusFile": "/run/momso-update/status.json", "publicKey": "/etc/momso-update-public.pem",
              "serverTokenFile": "/run/momso-vault/server-token", "lifecycleDir": "/var/lib/momso/lifecycle",
              "serverUnit": "momso-server.service", "proxyUnit": "momso-private-proxy.service",
              "project": provider.project, "region": REGION, "gatewayService": "momso-any-" + provider.install_id}
    payload = {"files": {name: base64.b64encode((source / name).read_bytes()).decode("ascii") for name in names},
               "proxyFiles": {name: base64.b64encode((Path(__file__).resolve().parent / name).read_bytes()).decode("ascii")
                              for name in ("momso_mobile_proxy.py", "momso_anywhere_vm_proxy.py")},
               "publicKey": base64.b64encode(Path(public_key).read_bytes()).decode("ascii"),
               "config": config, "manifest": manifest_url, "signature": signature}
    code = r'''data=json.load(sys.stdin);folder=pathlib.Path('/opt/momso-updater');folder.mkdir(mode=0o755,parents=True,exist_ok=True)
install_group(folder,'update-worker',{name:base64.b64decode(value,validate=True) for name,value in data['files'].items()},WORKER_LEGACY)
flat=pathlib.Path('/opt/momso');marker=flat/'releases/.momso-update-active';current=flat/'releases/current';override=pathlib.Path('/etc/systemd/system/momso-server.service.d/momso-update.conf')
active=marker.exists() or (override.is_file() and 'ExecStart=/opt/momso/releases/current/momso-server' in override.read_text())
if active:
 assert current.is_symlink() and current.resolve().is_relative_to((flat/'releases').resolve())
 proxy_dir=current.resolve()
else:proxy_dir=flat
assert set(data['proxyFiles'])=={'momso_mobile_proxy.py','momso_anywhere_vm_proxy.py'}
tag='update-proxy-'+hashlib.sha256(str(proxy_dir).encode()).hexdigest()[:16]
install_group(proxy_dir,tag,{name:base64.b64decode(value,validate=True) for name,value in data['proxyFiles'].items()},PROXY_LEGACY)
install_group(pathlib.Path('/etc'),'update-key',{'momso-update-public.pem':base64.b64decode(data['publicKey'],validate=True)},{})
sys.path.insert(0,str(folder))
import worker,install_worker,ensure_gcloud
ensure_gcloud.ensure()
poller=worker.make_poller(data['config'])
install_worker.install('/opt/momso','/etc/systemd/system',folder,'/etc/momso-update.json',data['config'],poller.updater,data['manifest'],data['signature'])
'''
    module = (Path(__file__).resolve().parent / 'momso_managed_files.py').read_bytes()
    bootstrap = r'''import base64,hashlib,json,os,pathlib,sys
os.makedirs('/etc/momso-private',mode=0o700,exist_ok=True)
if os.path.islink('/etc/momso-private'):raise RuntimeError('managed_directory_conflict')
managed={};exec(compile(base64.b64decode(MANAGED_MODULE),'momso_managed_files.py','exec'),managed)
groups=[]
def install_group(root,tag,items,legacy):
 manifest='/etc/momso-private/managed-'+tag+'.json';journal='/etc/momso-private/managed-'+tag+'-journal.json'
 managed['install'](root,manifest,journal,OWNER,items,legacy,0o644)
 groups.append((root,manifest,journal))
'''
    code = (bootstrap + 'try:\n' + textwrap.indent(code, ' ')
            + "\nexcept BaseException:\n for r,m,j in reversed(groups):managed['rollback'](r,m,j,OWNER)\n raise\n"
            + "for r,m,j in groups:os.unlink(j)\n")
    code = code.replace('MANAGED_MODULE', repr(base64.b64encode(module).decode('ascii'))).replace('OWNER', repr(provider.install_id)).replace('WORKER_LEGACY', repr(LEGACY_WORKER)).replace('PROXY_LEGACY', repr(LEGACY_PROXY))
    encoded = base64.b64encode(code.encode()).decode("ascii")
    wrapper = "import base64,subprocess,sys; subprocess.run(['sudo','python3','-c',base64.b64decode(" + repr(encoded) + ").decode()],input=sys.stdin.buffer.read(),check=True)"
    provider.ssh(wrapper, json.dumps(payload).encode())
