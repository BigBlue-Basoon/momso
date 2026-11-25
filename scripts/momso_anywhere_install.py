"""Install the customer-owned HTTPS gateway after the private VM exists."""
import base64
import hashlib
import hmac
import ipaddress
import importlib.util
import json
import re
import secrets
import subprocess
from types import SimpleNamespace
from pathlib import Path

from momso_server_install import InstallError, REGION, ZONE, save_state
from momso_wake_install import GoogleWake
from momso_iam_retry import bind_custom_role
from momso_anywhere_proxy_files import PROXY_FILES

_adapter_path = Path(__file__).resolve().parent.parent / 'apps/alpha/desktop/src-tauri/momso-server-update/cloud_run_gateway.py'
_adapter_spec = importlib.util.spec_from_file_location('momso_cloud_run_gateway', _adapter_path)
_adapter_module = importlib.util.module_from_spec(_adapter_spec)
_adapter_spec.loader.exec_module(_adapter_module)
CloudRunGateway = _adapter_module.CloudRunGateway
GatewayError = _adapter_module.GatewayError
gateway_failure = _adapter_module.gateway_failure


def cloud_run_gateway(provider, service):
    def run(command, **_kwargs):
        return SimpleNamespace(returncode=0, stdout=provider.run(*command[1:]))
    return CloudRunGateway(provider.project, REGION, service, runner=run)


def install_codex(provider):
    code = r'''import hashlib,io,os,shutil,subprocess,tarfile,urllib.request
root='/opt/momso';path=root+'/codex';marker=root+'/.codex-sha256'
def run(*args):subprocess.run(args,check=True,stdout=subprocess.DEVNULL,stderr=subprocess.DEVNULL)
if not (os.path.isfile(path) and os.path.isfile(marker) and hashlib.sha256(open(path,'rb').read()).hexdigest()==open(marker).read().strip()):
 with urllib.request.urlopen('https://github.com/openai/codex/releases/download/rust-v0.169.0/codex-x86_64-unknown-linux-musl.tar.gz',timeout=180) as r:data=r.read()
 assert hashlib.sha256(data).hexdigest()=='9af4d751fce183faedea629437d5252dbf458eeb04130cf4f97be844621ea985'
 with tarfile.open(fileobj=io.BytesIO(data),mode='r:gz') as tar:
  members=tar.getmembers();assert len(members)==1 and members[0].name=='codex-x86_64-unknown-linux-musl' and members[0].isfile()
  with tar.extractfile(members[0]) as src,open(path+'.new','wb') as dst:shutil.copyfileobj(src,dst)
 os.chmod(path+'.new',0o755);os.replace(path+'.new',path)
 with open(marker,'w') as f:f.write(hashlib.sha256(open(path,'rb').read()).hexdigest())
missing=[pkg for executable,pkg in (('bwrap','bubblewrap'),('git','git')) if not shutil.which(executable)]
if missing:
 os.environ['DEBIAN_FRONTEND']='noninteractive';run('apt-get','update','-qq');run('apt-get','install','-y','-qq',*missing)
env='/etc/momso-server.env'
lines=[line for line in open(env).read().splitlines() if not line.startswith('MOMSO_CODEX_PATH=')]
lines.append('MOMSO_CODEX_PATH=/opt/momso/codex')
with open(env,'w') as f:f.write('\n'.join(lines)+'\n')
run('systemctl','restart','momso-server')
'''
    wrapper = "import base64,subprocess; subprocess.run(['sudo','python3','-c',base64.b64decode(" + repr(base64.b64encode(code.encode()).decode()) + ").decode()],check=True)"
    provider.ssh(wrapper)


def install_private_proxy(provider):
    here = Path(__file__).resolve().parent
    from momso_managed_history import load as load_managed_history
    files = {name: base64.b64encode((here / name).read_bytes()).decode() for name in PROXY_FILES if name != 'server_admission.py'}
    files['server_admission.py'] = base64.b64encode((here.parent / 'apps/alpha/desktop/src-tauri/momso-server-update/server_admission.py').read_bytes()).decode()
    code = r'''import base64,hashlib,json,os,secrets,subprocess,sys
data=json.load(sys.stdin); root='/etc/momso-private';os.makedirs(root,mode=0o700,exist_ok=True);os.chmod(root,0o700)
cert=root+'/certificate.pem'; key=root+'/key.pem'; token=root+'/transport-token'
def run(*args):subprocess.run(args,check=True,stdout=subprocess.DEVNULL,stderr=subprocess.DEVNULL)
if not os.path.exists(cert) and not os.path.exists(key):
 run('openssl','req','-x509','-newkey','rsa:3072','-nodes','-days','3650','-subj','/CN=momso-private','-keyout',key,'-out',cert)
os.chmod(key,0o600)
if not os.path.exists(token):
 fd=os.open(token,os.O_WRONLY|os.O_CREAT|os.O_EXCL,0o600)
 with os.fdopen(fd,'w') as f:f.write(secrets.token_urlsafe(48))
base='/opt/momso';marker=base+'/releases/.momso-update-active';current=base+'/releases/current';override='/etc/systemd/system/momso-server.service.d/momso-update.conf'
active=os.path.exists(marker) or (os.path.isfile(override) and 'ExecStart=/opt/momso/releases/current/momso-server' in open(override).read())
if active:
 assert (not os.path.exists(marker) or (not os.path.islink(marker) and open(marker).read().strip()=='momso-update-v1')) and os.path.islink(current)
 target=os.path.realpath(current);assert os.path.commonpath((target,base+'/releases'))==base+'/releases' and target!=base+'/releases'
 base=target
os.makedirs(base,exist_ok=True)
manifest='/etc/momso-private/managed-update-proxy-'+hashlib.sha256(base.encode()).hexdigest()[:16]+'.json'
recorded={}
if os.path.exists(manifest):
 assert not os.path.islink(manifest)
 ledger=json.load(open(manifest));assert ledger.get('version')==1 and ledger.get('owner')==OWNER and isinstance(ledger.get('files'),dict)
 recorded=ledger['files']
for name,value in data.items():
 path=base+'/'+name
 if os.path.lexists(path):
  assert os.path.isfile(path) and not os.path.islink(path)
  old=hashlib.sha256(open(path,'rb').read()).hexdigest();new=hashlib.sha256(base64.b64decode(value,validate=True)).hexdigest()
  assert old==new or old==recorded.get(name) or old in LEGACY.get(name,[])
for name,value in data.items():
 path=base+'/'+name;temp=path+'.new'
 with open(temp,'wb') as f:f.write(base64.b64decode(value,validate=True))
 os.chmod(temp,0o755);os.replace(temp,path)
unit='[Unit]\nDescription=momso private proxy\nAfter=momso-server.service\n'+('' if base!='/opt/momso' else 'Requires=momso-server.service\n')+'[Service]\nLoadCredential=server-env:/etc/momso-server.env\nEnvironment=MOMSO_MOBILE_HOST=private.momso\nEnvironment=MOMSO_PRIVATE_PROXY_TOKEN_FILE=/etc/momso-private/transport-token\nEnvironment=MOMSO_PRIVATE_CERT=/etc/momso-private/certificate.pem\nEnvironment=MOMSO_PRIVATE_KEY=/etc/momso-private/key.pem\nExecStart=/usr/bin/python3 '+('/opt/momso/releases/current' if base!='/opt/momso' else base)+'/momso_anywhere_vm_proxy.py\nNoNewPrivileges=true\nPrivateTmp=true\nRestart=on-failure\n[Install]\nWantedBy=multi-user.target\n'
with open('/etc/systemd/system/momso-private-proxy.service','w') as f:f.write(unit)
run('systemctl','daemon-reload');run('systemctl','enable','--now','momso-private-proxy');run('systemctl','restart','momso-private-proxy')
import ssl,time,urllib.error,urllib.request
ctx=ssl._create_unverified_context()
deadline=time.monotonic()+30
while True:
 try:
  with open(token) as f: private_token=f.read().strip()
  req=urllib.request.Request('https://127.0.0.1:8766/health',headers={'Authorization':'Bearer '+private_token})
  with urllib.request.urlopen(req,context=ctx,timeout=3) as response:
   if json.load(response).get('state')=='ready':break
 except (OSError,urllib.error.URLError):pass
 if time.monotonic()>=deadline:raise RuntimeError('private_proxy_not_ready')
 time.sleep(1)
der=subprocess.check_output(['openssl','x509','-in',cert,'-outform','DER'])
print(json.dumps({'fingerprint':hashlib.sha256(der).hexdigest(),'token':open(token).read().strip()}))
'''
    code = code.replace('OWNER', repr(provider.install_id)).replace('LEGACY', repr(load_managed_history()['proxy']))
    wrapper = "import base64,subprocess,sys; subprocess.run(['sudo','python3','-c',base64.b64decode(" + repr(base64.b64encode(code.encode()).decode()) + ").decode()],input=sys.stdin.buffer.read(),check=True)"
    try:
        value = json.loads(provider.ssh(wrapper, json.dumps(files).encode()))
        if not re.fullmatch(r"[0-9a-f]{64}", value.get("fingerprint", "")) or not re.fullmatch(r"[A-Za-z0-9_-]{32,}", value.get("token", "")):
            raise ValueError("invalid proxy identity")
        return value
    except (ValueError, TypeError, json.JSONDecodeError) as error:
        raise InstallError("VM의 인증서와 전송 권한을 확인하지 못했습니다.") from error


class GoogleAnywhere(GoogleWake):
    def __init__(self, provider):
        super().__init__(provider)
        from momso_anywhere_vm_resources import names
        resources = names(provider)
        self.service = resources['gateway']
        self.sa = resources['gateway_account']
        self.email = self.sa + "@" + provider.project + ".iam.gserviceaccount.com"
        self.role = resources['gateway_role']
        self.transport_secret = resources['transport_secret']
        self.session_secret = resources['session_secret']
        self._first_attempt_absent = False

    def ensure_secret(self, name, value=None):
        p = self.p
        items = p.run("secrets", "list", parse=True)
        found = next((x for x in items if x.get("name", "").endswith("/secrets/" + name)), None)
        if found and found.get("labels", {}).get("momso-install-id") != p.install_id:
            raise InstallError("기존 비밀 저장소와 이름이 겹칩니다.")
        if not found:
            p.run("secrets", "create", name, "--replication-policy=automatic", f"--labels=momso-install-id={p.install_id}")
        versions = p.run("secrets", "versions", "list", name, parse=True)
        if not versions:
            actual = value or secrets.token_urlsafe(48).encode()
            p.run("secrets", "versions", "add", name, "--data-file=-", data=actual)
        elif value is not None:
            actual = p.run("secrets", "versions", "access", "latest", f"--secret={name}").strip()
            if not hmac.compare_digest(actual, value):
                raise InstallError("VM과 비밀 저장소의 전송 권한이 다릅니다. 기존 값을 덮어쓰지 않았습니다.")
        bind_custom_role(p, "secrets", "add-iam-policy-binding", name, f"--member=serviceAccount:{self.email}", "--role=roles/secretmanager.secretAccessor")

    def activate_gateway(self, image, *, flags=(), previous=True, expected_url=None, health_check=None):
        p = self.p
        adapter = cloud_run_gateway(p, self.service)
        try:
            if not previous:
                previous = self._objects()[2] is not None
                self._first_attempt_absent = not previous
            before = adapter.current() if previous else None
        except (GatewayError, InstallError, OSError) as error:
            raise InstallError("기존 입구의 revision을 확인하지 못했습니다.", gateway_failure=gateway_failure(str(error) if isinstance(error, GatewayError) else 'gateway_operation_failed')) from error
        try:
            adapter.activate(image, deploy_flags=flags, first_deploy=not previous, probe='--no-allow-unauthenticated' not in flags)
            if not adapter.healthy():
                raise GatewayError('gateway_not_ready')
            if expected_url:
                current = p.run("run", "services", "describe", self.service, f"--region={REGION}", parse=True)
                if current.get("status", {}).get("url") != expected_url:
                    raise GatewayError('gateway_address_changed')
            if health_check is not None:
                health_check()
        except (GatewayError, InstallError, OSError) as error:
            if before:
                try:
                    adapter.activate(before)
                except (GatewayError, InstallError, OSError) as rollback_error:
                    raise InstallError("입구 갱신과 이전 판 복구에 실패했습니다. 기존 revision을 확인하세요.", gateway_failure=gateway_failure('gateway_rollback_failed', False)) from rollback_error
            code = str(error) if isinstance(error, GatewayError) else getattr(error, 'gateway_code', None) or 'gateway_operation_failed'
            if code == 'proxy_not_running':
                raise InstallError('proxy_not_running', gateway_code=code, gateway_failure={'code': code, 'rolledBack': True if before else None, 'retryable': True if before else None}) from error
            raise InstallError("입구 갱신에 실패했습니다. 기존 판을 유지했는지 확인하세요.", gateway_failure=gateway_failure(code, True if before else None)) from error

    def upgrade_gateway(self, expected_url, image, *, health_check=None):
        self.activate_gateway(image, expected_url=expected_url, health_check=health_check)

    def install(self, proxy, client_id, owner_sub, login_origin, image):
        p = self.p
        if not re.fullmatch(r"[A-Za-z0-9._-]+\.apps\.googleusercontent\.com", client_id) or not re.fullmatch(r"[0-9]{1,40}", owner_sub) or not re.fullmatch(r"https://[a-z0-9][a-z0-9.-]*", login_origin):
            raise InstallError("Google 로그인 앱 ID와 계정의 고유 번호를 확인하세요.")
        existing = self.prepare_identity()
        if existing:
            previous = p.run("run", "services", "describe", self.service, f"--region={REGION}", parse=True)
            containers = previous.get("spec", {}).get("template", {}).get("spec", {}).get("containers", [])
            old_pin = next((item.get("value") for container in containers for item in container.get("env", []) if item.get("name") == "MOMSO_PRIVATE_CERT_SHA256"), None)
            if old_pin and not hmac.compare_digest(old_pin, proxy["fingerprint"]):
                raise InstallError("VM 인증서 지문이 달라 입구를 바꾸지 않았습니다.")
        self.ensure_secret(self.transport_secret, proxy["token"].encode())
        self.ensure_secret(self.session_secret)
        from momso_anywhere_vm_resources import names
        rule = names(p)['private_firewall']
        p.ensure("사설 통로 방화벽", rule, ["compute", "firewall-rules", "list"], ["compute", "firewall-rules", "create", rule, f"--network={p.network}", "--direction=INGRESS", "--action=ALLOW", "--rules=tcp:8766", "--source-ranges=10.42.0.0/24", f"--target-tags={p.name}", f"--description={p.marker}"])
        firewall = p.run("compute", "firewall-rules", "describe", rule, parse=True)
        if firewall.get("description") != p.marker or firewall.get("direction") != "INGRESS" or firewall.get("disabled") or not firewall.get("network", "").endswith("/" + p.network) or firewall.get("sourceRanges") != ["10.42.0.0/24"] or firewall.get("targetTags") != [p.name] or firewall.get("allowed") != [{"IPProtocol": "tcp", "ports": ["8766"]}]:
            raise InstallError("사설 통로 방화벽의 범위가 달라 입구를 열지 않았습니다.")
        instance = p.run("compute", "instances", "describe", p.name, f"--zone={ZONE}", parse=True)
        ip = instance.get("networkInterfaces", [{}])[0].get("networkIP", "")
        try:
            valid_ip = ipaddress.ip_address(ip) in ipaddress.ip_network("10.42.0.0/24")
        except ValueError:
            valid_ip = False
        if not valid_ip:
            raise InstallError("VM의 사설 주소를 확인하지 못했습니다.")
        env = f"MOMSO_PRIVATE_HOST={ip},MOMSO_PRIVATE_PORT=8766,MOMSO_PRIVATE_CERT_SHA256={proxy['fingerprint']},MOMSO_PROJECT={p.project},MOMSO_ZONE={ZONE},MOMSO_INSTANCE={p.name},MOMSO_GOOGLE_CLIENT_ID={client_id},MOMSO_OWNER_SUB={owner_sub},MOMSO_LOGIN_ORIGIN={login_origin},MOMSO_SESSION_KEY_FILE=/secrets/session-key"
        secret_flags = f"--set-secrets=MOMSO_PRIVATE_PROXY_TOKEN={self.transport_secret}:latest,/secrets/session-key={self.session_secret}:latest"
        common = (f"--service-account={self.email}", f"--labels=momso-install-id={p.install_id}", f"--network={p.network}", f"--subnet={p.subnet}", "--vpc-egress=private-ranges-only", "--timeout=3600", secret_flags)
        try:
            self.activate_gateway(image, flags=(*common, "--no-allow-unauthenticated", f"--set-env-vars={env},MOMSO_PUBLIC_HOST=pending.invalid"), previous=bool(existing))
            details = p.run("run", "services", "describe", self.service, f"--region={REGION}", parse=True)
            url = details.get("status", {}).get("url", "")
            if not re.fullmatch(r"https://[a-z0-9-]+\.[a-z0-9.-]+\.run\.app", url):
                raise InstallError("입구 주소를 확인하지 못했습니다.")
            host = url.removeprefix("https://")
            self.activate_gateway(image, flags=(*common, "--allow-unauthenticated", f"--set-env-vars={env},MOMSO_PUBLIC_HOST={host}"), expected_url=url)
            return url
        except (InstallError, OSError, subprocess.SubprocessError):
            if not existing and self._first_attempt_absent:
                try:
                    service = self._objects()[2]
                    if service: p.run('run', 'services', 'delete', self.service, f'--region={REGION}')
                except (InstallError, OSError, subprocess.SubprocessError) as cleanup_error:
                    raise InstallError('새 입구를 확인하지 못했고 정리도 확인하지 못했습니다.', gateway_failure=gateway_failure('gateway_rollback_failed', False)) from cleanup_error
            raise

    def destroy(self, *, keep_roles=False):
        p = self.p
        self._objects()
        from momso_server_install import owned_listing
        items = owned_listing(p, "secrets", "list")
        owned = []
        for name in (self.transport_secret, self.session_secret):
            found = next((x for x in items if x.get("name", "").endswith("/secrets/" + name)), None)
            if found and found.get("labels", {}).get("momso-install-id") != p.install_id:
                raise InstallError("비밀 저장소의 소유권이 달라 삭제하지 않았습니다.")
            if found:
                owned.append(name)
        from momso_anywhere_vm_resources import names
        rule = names(p)['private_firewall']
        rules = owned_listing(p, "compute", "firewall-rules", "list", f"--filter=name={rule}")
        matching = next((x for x in rules if x.get("name") == rule), None)
        if matching and matching.get("description") != p.marker:
            raise InstallError("방화벽의 소유권이 달라 삭제하지 않았습니다.")
        super().destroy(keep_roles=keep_roles)
        for name in owned:
            p.run("secrets", "delete", name)
        if matching:
            p.run("compute", "firewall-rules", "delete", rule)


def install(provider, state_path, state, client_id, owner_sub, login_origin, image, *, signed_files=None, preserve_destroyed=False):
    setup = state.setdefault("anywhere", {"client_id": client_id, "owner_sub": owner_sub, "login_origin": login_origin})
    if setup.get("client_id") != client_id or setup.get("owner_sub") != owner_sub or setup.get("login_origin") != login_origin:
        raise InstallError("기존 설치의 Google 로그인 정보가 다릅니다.")
    save_state(state_path, state)
    install_codex(provider)
    import momso_anywhere_vm_services
    if signed_files is None:
        momso_anywhere_vm_services.install(provider)
    else:
        momso_anywhere_vm_services.install(provider, signed_files=signed_files)
    proxy = install_private_proxy(provider)
    gateway = GoogleAnywhere(provider)
    setup["url"] = gateway.install(proxy, client_id, owner_sub, login_origin, image)
    if not preserve_destroyed:
        setup.pop("destroyed", None)
    setup['gatewayCreatedInAttempt'] = gateway._first_attempt_absent
    save_state(state_path, state)
    return setup["url"]
