"""Cloudflare Access and Tunnel provisioning for one private momso server."""
from __future__ import annotations

import base64
import hashlib
import json
import os
import re
import urllib.error
import urllib.parse
import urllib.request
from pathlib import Path

from momso_server_install import InstallError, save_state

RELEASE = "https://github.com/cloudflare/cloudflared/releases/download/2026.10.6/cloudflared-linux-amd64"
RELEASE_SHA256 = "1d356dac769d0dbf62ebe588c4034824bbff98d5790c2f844cc99e0e2f57462d"
API = "https://api.cloudflare.com/client/v4"


class NoRedirect(urllib.request.HTTPRedirectHandler):
    def redirect_request(self, *args, **kwargs):
        return None


class Cloudflare:
    def __init__(self, token: str, account: str, zone: str, transport=None):
        if not token or not re.fullmatch(r"[a-f0-9]{32}", account) or not re.fullmatch(r"[a-f0-9]{32}", zone):
            raise InstallError("Cloudflare 계정·영역·권한을 확인하세요.")
        self.token, self.account, self.zone = token, account, zone
        self.transport = transport or self._request

    def _request(self, method, path, body=None):
        data = json.dumps(body).encode() if body is not None else None
        request = urllib.request.Request(API + path, data=data, method=method, headers={"Authorization": "Bearer " + self.token, "Content-Type": "application/json"})
        try:
            with urllib.request.build_opener(NoRedirect).open(request, timeout=45) as response:
                raw = response.read(4 * 1024 * 1024 + 1)
            if len(raw) > 4 * 1024 * 1024:
                raise ValueError("response too large")
            result = json.loads(raw)
            if not result.get("success"):
                raise ValueError("API failure")
            return result
        except (OSError, ValueError, urllib.error.URLError) as error:
            raise InstallError("Cloudflare 요청에 실패했습니다. 계정·도메인·API 권한을 확인하세요.") from error

    def call(self, method, path, body=None):
        return self.transport(method, path, body)["result"]

    def find(self, path, name):
        result = []
        for page in range(1, 101):
            sep = "&" if "?" in path else "?"
            reply = self.transport("GET", f"{path}{sep}per_page=100&page={page}", None)
            values = reply.get("result", [])
            if not isinstance(values, list):
                raise InstallError("Cloudflare 목록을 확인하지 못했습니다.")
            result.extend(value for value in values if value.get("name") == name and not value.get("deleted_at"))
            info = reply.get("result_info") or {}
            total = info.get("total_pages")
            if total is not None and page >= total:
                break
            if total is None and len(values) < 100:
                break
        else:
            raise InstallError("Cloudflare 목록이 너무 길어 확인을 멈췄습니다.")
        if len(result) > 1:
            raise InstallError("같은 이름의 Cloudflare 자원이 둘 이상입니다.")
        return result[0] if result else None


def validate_policy(policies, email):
    return isinstance(policies, list) and len(policies) == 1 and policies[0].get("decision") == "allow" and policies[0].get("include") == [{"email": {"email": email}}] and not policies[0].get("exclude") and not policies[0].get("require")


def destroy_owned(cf, state_path: Path, state: dict, hostname: str, email: str, destroy_vm):
    """Remove only this installation's resources; keep Access until the tunnel is gone."""
    mobile = state.get("mobile")
    binding = hashlib.sha256(json.dumps([cf.account, cf.zone, hostname, email], separators=(",", ":")).encode()).hexdigest()
    if not isinstance(mobile, dict) or mobile.get("binding") != binding:
        raise InstallError("이 설치 기록의 Cloudflare 설정과 다릅니다.")
    name = "momso-" + state["id"]
    base = f"/accounts/{cf.account}"
    apps, tunnels, records = base + "/access/apps", base + "/cfd_tunnel", f"/zones/{cf.zone}/dns_records"
    app = cf.find(apps + "?" + urllib.parse.urlencode({"name": name, "exact": "true"}), name)
    tunnel = cf.find(tunnels + "?" + urllib.parse.urlencode({"name": name, "is_deleted": "false"}), name)
    record = cf.find(records + "?" + urllib.parse.urlencode({"name": hostname}), hostname)
    if app and ((app.get("id") != mobile.get("app_id") and not mobile.get("creating_app")) or app.get("domain") != hostname):
        raise InstallError("Access 앱의 소유권이 달라 삭제하지 않았습니다.")
    if tunnel and ((tunnel.get("id") != mobile.get("tunnel_id") and not mobile.get("creating_tunnel")) or tunnel.get("config_src") != "cloudflare"):
        raise InstallError("터널의 소유권이 달라 삭제하지 않았습니다.")
    tunnel_id = mobile.get("tunnel_id") or (tunnel or {}).get("id")
    if record and ((record.get("id") != mobile.get("dns_id") and not mobile.get("creating_dns")) or record.get("comment") != name or record.get("type") != "CNAME" or record.get("content") != str(tunnel_id) + ".cfargotunnel.com"):
        raise InstallError("DNS 기록의 소유권이 달라 삭제하지 않았습니다.")
    policies = []
    if app:
        reply = cf.transport("GET", apps + "/" + app["id"] + "/policies?per_page=100&page=1", None)
        policies = reply.get("result", [])
        known = mobile.get("policy_ids", [])
        pending = mobile.get("creating_app") and validate_policy(policies, email) and all(item.get("name") == name and re.fullmatch(r"[a-f0-9-]{36}", item.get("id", "")) for item in policies)
        if (reply.get("result_info") or {}).get("total_pages", 1) > 1 or not isinstance(policies, list) or (not pending and any(item.get("id") not in known for item in policies)):
            raise InstallError("Access 앱의 정책 소유권을 확인하지 못해 삭제하지 않았습니다.")
    if record:
        cf.call("DELETE", records + "/" + record["id"])
    destroy_vm()
    if tunnel:
        cf.call("DELETE", tunnels + "/" + tunnel["id"] + "/connections")
        cf.call("DELETE", tunnels + "/" + tunnel["id"])
    if app:
        for policy in policies:
            cf.call("DELETE", apps + "/" + app["id"] + "/policies/" + policy["id"])
        cf.call("DELETE", apps + "/" + app["id"])
    mobile["destroyed"] = True
    save_state(state_path, state)


def configure(cf, state_path: Path, state: dict, hostname: str, email: str):
    if not re.fullmatch(r"(?=.{1,253}$)[a-z0-9](?:[a-z0-9.-]*[a-z0-9])?", hostname) or ".." in hostname:
        raise InstallError("접속 주소를 확인하세요.")
    if not re.fullmatch(r"[^\s@]+@[^\s@]+\.[^\s@]+", email):
        raise InstallError("허용할 이메일을 확인하세요.")
    binding = hashlib.sha256(json.dumps([cf.account, cf.zone, hostname, email], separators=(",", ":")).encode()).hexdigest()
    mobile = state.setdefault("mobile", {"binding": binding})
    if mobile.get("binding") != binding:
        raise InstallError("이 설치 기록의 Cloudflare 설정과 다릅니다.")
    mobile.pop("destroyed", None)
    save_state(state_path, state)
    name = "momso-" + state["id"]
    base = f"/accounts/{cf.account}"
    zone = cf.call("GET", f"/zones/{cf.zone}")
    if not isinstance(zone, dict) or zone.get("status") != "active" or zone.get("account", {}).get("id") != cf.account or not hostname.endswith("." + str(zone.get("name", ""))):
        raise InstallError("활성 Cloudflare 도메인과 접속 주소가 맞지 않습니다.")
    organization = cf.call("GET", base + "/access/organizations")
    auth_domain = organization.get("auth_domain", "") if isinstance(organization, dict) else ""
    if not re.fullmatch(r"[a-z0-9-]+\.cloudflareaccess\.com", auth_domain):
        raise InstallError("Cloudflare Access 조직을 먼저 설정하세요.")
    team = auth_domain.removesuffix(".cloudflareaccess.com")
    apps = base + "/access/apps"
    # Never create an Access application over another application's hostname.
    domain_query = apps + "?" + urllib.parse.urlencode({"domain": hostname, "exact": "true", "per_page": 100})
    matching_apps = cf.transport("GET", domain_query, None)
    if (matching_apps.get("result_info") or {}).get("total_pages", 1) > 1 or any(item.get("name") != name for item in matching_apps.get("result", [])):
        raise InstallError("접속 주소를 이미 사용하는 Access 앱이 있어 변경하지 않았습니다.")
    records = f"/zones/{cf.zone}/dns_records"
    record = cf.find(records + "?" + urllib.parse.urlencode({"name": hostname}), hostname)
    marker = "momso-" + state["id"]
    if record and record.get("comment") != marker:
        raise InstallError("기존 DNS 기록과 겹쳐 변경하지 않았습니다.")
    app = cf.find(apps + "?" + urllib.parse.urlencode({"name": name, "exact": "true"}), name)
    if app and app.get("id") != mobile.get("app_id") and not mobile.get("creating_app"):
        raise InstallError("같은 이름의 기존 Access 앱은 변경하지 않았습니다.")
    if not app:
        mobile["creating_app"] = True
        save_state(state_path, state)
        app = cf.call("POST", apps, {"name": name, "domain": hostname, "type": "self_hosted", "session_duration": "24h", "policies": [{"name": name, "decision": "allow", "include": [{"email": {"email": email}}]}]})
    if not isinstance(app, dict) or app.get("name") != name or app.get("domain") != hostname or not re.fullmatch(r"[a-f0-9]{64}", app.get("aud", "")):
        raise InstallError("Access 앱과 접속 주소를 확인하지 못했습니다.")
    app_id = app.get("id", "")
    if not re.fullmatch(r"[a-f0-9-]{36}", app_id):
        raise InstallError("Access 앱 식별자를 확인하지 못했습니다.")
    policy_reply = cf.transport("GET", apps + f"/{app_id}/policies?per_page=100&page=1", None)
    policies = policy_reply.get("result")
    if (policy_reply.get("result_info") or {}).get("total_pages", 1) > 1 or not validate_policy(policies, email):
        raise InstallError("Access 허용 정책이 요청한 한 사람과 다릅니다. 연결을 열지 않았습니다.")
    ids = [item.get("id", "") for item in policies]
    if not all(re.fullmatch(r"[a-f0-9-]{36}", value) for value in ids):
        raise InstallError("Access 정책 식별자를 확인하지 못했습니다.")
    mobile["policy_ids"] = ids
    mobile["app_id"] = app_id
    mobile.pop("creating_app", None)
    save_state(state_path, state)
    tunnels = base + "/cfd_tunnel"
    tunnel = cf.find(tunnels + "?" + urllib.parse.urlencode({"name": name, "is_deleted": "false"}), name)
    if tunnel and tunnel.get("id") != mobile.get("tunnel_id") and not mobile.get("creating_tunnel"):
        raise InstallError("같은 이름의 기존 터널은 변경하지 않았습니다.")
    if not tunnel:
        mobile["creating_tunnel"] = True
        save_state(state_path, state)
        tunnel = cf.call("POST", tunnels, {"name": name, "config_src": "cloudflare"})
    tunnel_id = tunnel.get("id", "") if isinstance(tunnel, dict) else ""
    if not isinstance(tunnel, dict) or tunnel.get("name") != name or not re.fullmatch(r"[a-f0-9-]{36}", tunnel_id) or tunnel.get("config_src") != "cloudflare":
        raise InstallError("터널 식별자를 확인하지 못했습니다.")
    mobile["tunnel_id"] = tunnel_id
    mobile.pop("creating_tunnel", None)
    save_state(state_path, state)
    config_path = tunnels + f"/{tunnel_id}/configurations"
    desired = {"ingress": [{"hostname": hostname, "service": "unix:/run/momso-mobile/proxy.sock", "originRequest": {"access": {"required": True, "teamName": team, "audTag": [app["aud"]]}}}, {"service": "http_status:404"}]}
    cf.call("PUT", config_path, {"config": desired})
    actual = cf.call("GET", config_path)
    ingress = actual.get("config", {}).get("ingress", []) if isinstance(actual, dict) else []
    actual_access = ingress[0].get("originRequest", {}).get("access", {}) if ingress else {}
    if len(ingress) != 2 or ingress[0].get("hostname") != hostname or ingress[0].get("service") != desired["ingress"][0]["service"] or actual_access.get("required") is not True or actual_access.get("teamName") != team or actual_access.get("audTag") != [app["aud"]] or ingress[1].get("service") != "http_status:404":
        raise InstallError("터널의 Access 검증을 확인하지 못해 시작하지 않았습니다.")
    expected = tunnel_id + ".cfargotunnel.com"
    if record and record.get("id") != mobile.get("dns_id") and not mobile.get("creating_dns"):
        raise InstallError("같은 이름의 기존 DNS 기록은 변경하지 않았습니다.")
    if not record:
        mobile["creating_dns"] = True
        save_state(state_path, state)
        record = cf.call("POST", records, {"type": "CNAME", "name": hostname, "content": expected, "proxied": True, "comment": marker})
    if not isinstance(record, dict) or record.get("name") != hostname or record.get("content") != expected or record.get("type") != "CNAME" or not record.get("proxied") or record.get("comment") != marker:
        raise InstallError("기존 DNS 기록과 겹쳐 변경하지 않았습니다.")
    mobile["dns_id"] = record.get("id")
    mobile.pop("creating_dns", None)
    save_state(state_path, state)
    token = cf.call("GET", tunnels + f"/{tunnel_id}/token")
    if not isinstance(token, str) or not token or "\n" in token:
        raise InstallError("터널 연결 권한을 확인하지 못했습니다.")
    return token, auth_domain


def download_connector():
    try:
        with urllib.request.urlopen(RELEASE, timeout=120) as response:
            binary = response.read()
    except OSError as error:
        raise InstallError("터널 프로그램을 받지 못했습니다.") from error
    if hashlib.sha256(binary).hexdigest() != RELEASE_SHA256:
        raise InstallError("터널 프로그램의 SHA-256이 다릅니다.")
    return binary


def install_vm(provider, tunnel_token: str, binary: bytes, hostname: str):
    if hashlib.sha256(binary).hexdigest() != RELEASE_SHA256:
        raise InstallError("터널 프로그램의 SHA-256이 다릅니다.")
    proxy = Path(__file__).with_name("momso_mobile_proxy.py").read_bytes()
    data = json.dumps({"tunnel": tunnel_token, "hostname": hostname, "connector": base64.b64encode(binary).decode(), "proxy": base64.b64encode(proxy).decode()}).encode()
    payload = r'''import base64,hashlib,json,os,subprocess,sys
data=json.load(sys.stdin)
binary=base64.b64decode(data['connector'],validate=True)
assert hashlib.sha256(binary).hexdigest()==EXPECTED
def run(*args): subprocess.run(args,check=True,stdout=subprocess.DEVNULL)
if subprocess.run(['getent','group','momso-mobile'],stdout=subprocess.DEVNULL).returncode: run('groupadd','--system','momso-mobile')
for user in ('momso-mobile-proxy','momso-tunnel'):
 if subprocess.run(['id',user],stdout=subprocess.DEVNULL,stderr=subprocess.DEVNULL).returncode: run('useradd','--system','--no-create-home','--shell','/usr/sbin/nologin','--gid','momso-mobile',user)
import grp,pwd
group=grp.getgrnam('momso-mobile'); allowed={'momso-mobile-proxy','momso-tunnel'}
assert set(group.gr_mem).issubset(allowed)
assert all(user.pw_name in allowed for user in pwd.getpwall() if user.pw_gid==group.gr_gid)
assert all(pwd.getpwnam(user).pw_gid==group.gr_gid and pwd.getpwnam(user).pw_uid!=0 and pwd.getpwnam(user).pw_shell=='/usr/sbin/nologin' for user in allowed)
run('install','-d','-m','0755','/opt/momso')
def write_atomic(path,content,mode):
 tmp=path+'.new'
 fd=os.open(tmp,os.O_WRONLY|os.O_CREAT|os.O_TRUNC,mode)
 with os.fdopen(fd,'wb') as f: f.write(content);f.flush();os.fsync(f.fileno())
 os.chmod(tmp,mode);os.replace(tmp,path)
write_atomic('/opt/momso/cloudflared',binary,0o755)
write_atomic('/opt/momso/mobile-proxy.py',base64.b64decode(data['proxy'],validate=True),0o755)
write_atomic('/etc/momso-tunnel.token',data['tunnel'].encode(),0o600)
proxy='[Unit]\nDescription=momso mobile local proxy\nAfter=momso-server.service\nRequires=momso-server.service\n[Service]\nUser=momso-mobile-proxy\nGroup=momso-mobile\nRuntimeDirectory=momso-mobile\nRuntimeDirectoryMode=0750\nLoadCredential=server-env:/etc/momso-server.env\nEnvironment=MOMSO_MOBILE_HOST='+data['hostname']+'\nExecStart=/usr/bin/python3 /opt/momso/mobile-proxy.py\nRestart=on-failure\n[Install]\nWantedBy=multi-user.target\n'
tunnel='[Unit]\nDescription=momso Access tunnel\nAfter=network-online.target momso-mobile-proxy.service\nRequires=momso-mobile-proxy.service\n[Service]\nUser=momso-tunnel\nGroup=momso-mobile\nLoadCredential=tunnel-token:/etc/momso-tunnel.token\nExecStart=/opt/momso/cloudflared tunnel --no-autoupdate run --token-file %d/tunnel-token\nRestart=on-failure\n[Install]\nWantedBy=multi-user.target\n'
write_atomic('/etc/systemd/system/momso-mobile-proxy.service',proxy.encode(),0o644)
write_atomic('/etc/systemd/system/momso-tunnel.service',tunnel.encode(),0o644)
run('systemctl','daemon-reload');run('systemctl','enable','--now','momso-mobile-proxy');run('systemctl','restart','momso-mobile-proxy');run('systemctl','enable','--now','momso-tunnel');run('systemctl','restart','momso-tunnel')
'''.replace("EXPECTED", repr(RELEASE_SHA256))
    encoded = base64.b64encode(payload.encode()).decode()
    wrapper = "import base64,subprocess,sys\np=" + repr(encoded) + "\nsubprocess.run(['sudo','python3','-c',base64.b64decode(p).decode()],input=sys.stdin.buffer.read(),check=True)\n"
    provider.ssh(wrapper, data)


def connect(provider, cf, state_path, state, hostname, email, connector=None):
    tunnel_token, auth_domain = configure(cf, state_path, state, hostname, email)
    binary = connector if connector is not None else download_connector()
    install_vm(provider, tunnel_token, binary, hostname)
    ready = r'''import socket,subprocess
subprocess.run(['systemctl','is-active','--quiet','momso-tunnel'],check=True)
code="import socket; s=socket.socket(socket.AF_UNIX); s.settimeout(10); s.connect('/run/momso-mobile/proxy.sock'); s.sendall(b'GET /health HTTP/1.0\\r\\nHost: HOST\\r\\n\\r\\n'); r=b''\nwhile True:\n c=s.recv(4096)\n if not c: break\n r+=c\nassert b' 200 ' in r.split(b'\\r\\n',1)[0] and b'\\\"state\\\"' in r and b'ready' in r"
subprocess.run(['sudo','python3','-c',code],check=True)
'''.replace("HOST", hostname)
    provider.ssh(ready)
    def stop_tunnel():
        try:
            provider.ssh("import subprocess; subprocess.run(['sudo','systemctl','stop','momso-tunnel'],check=True)")
        except Exception as error:
            raise InstallError("접속 인증을 확인하지 못했고 터널 중지도 실패했습니다. 즉시 VM을 확인하세요.") from error
    # Check only an unauthenticated request, without following redirects.
    request = urllib.request.Request("https://" + hostname + "/health", method="GET")
    try:
        with urllib.request.build_opener(NoRedirect).open(request, timeout=20):
            stop_tunnel()
            raise InstallError("인증 없는 요청이 허용되어 터널을 중지했습니다.")
    except urllib.error.HTTPError as error:
        location = urllib.parse.urlparse(error.headers.get("Location", ""))
        if error.code not in (401, 403) and not (error.code in (301, 302, 303, 307, 308) and location.scheme == "https" and location.hostname == auth_domain):
            stop_tunnel()
            raise InstallError("Access 인증 확인이 예상과 다릅니다.") from error
    except urllib.error.URLError as error:
        stop_tunnel()
        raise InstallError("접속 주소가 아직 준비되지 않았습니다. 잠시 뒤 다시 실행하세요.") from error
    state["mobile"]["configured"] = True
    save_state(state_path, state)
