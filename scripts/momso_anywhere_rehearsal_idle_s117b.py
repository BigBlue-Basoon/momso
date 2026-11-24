"""Disposable-install idle/wake probe. No account credentials are collected."""
from __future__ import annotations

import base64
import json
import re
import time
import urllib.error
import urllib.parse
import urllib.request

from momso_server_install import REGION, ZONE

def _validate_target(provider):
    identity = getattr(provider, 'install_id', None)
    if not isinstance(identity, str) or not re.fullmatch(r'[a-f0-9]{12}', identity) or identity in getattr(provider, 'protected_installation_ids', ()):
        raise ValueError('invalid_installation_id')


# Only an independently verified, disposable VM is passed here. The exact
# original ExecStart is checked before the drop-in replaces it.
_OVERRIDE = r'''import os,pathlib,subprocess
unit=pathlib.Path('/etc/systemd/system/momso-idle-stop.service')
expected='ExecStart=/usr/bin/python3 /opt/momso/idle-stop.py --token-file ${CREDENTIALS_DIRECTORY}/server-token --state-dir /var/lib/momso/lifecycle'
text=unit.read_text(encoding='utf-8').splitlines()
assert text.count(expected)==1 and 'User=momso' in text
directory=pathlib.Path('/etc/systemd/system/momso-idle-stop.service.d')
assert not directory.is_symlink()
directory.mkdir(mode=0o755,exist_ok=True)
target=directory/'90-momso-rehearsal.conf'
assert not target.exists() and not target.is_symlink()
flags=os.O_WRONLY|os.O_CREAT|os.O_EXCL|getattr(os,'O_NOFOLLOW',0)
with os.fdopen(os.open(target,flags,0o644),'w',encoding='utf-8') as f:
 f.write('[Service]\nExecStart=\n'+expected+' --idle-seconds 60 --poll-seconds 5\n')
subprocess.run(['systemctl','daemon-reload'],check=True,stdout=subprocess.DEVNULL,stderr=subprocess.DEVNULL)
subprocess.run(['systemctl','restart','momso-idle-stop.service'],check=True,stdout=subprocess.DEVNULL,stderr=subprocess.DEVNULL)
effective=subprocess.check_output(['systemctl','show','momso-idle-stop.service','--property=ExecStart','--value'],text=True)
assert '--idle-seconds 60' in effective and '--poll-seconds 5' in effective
print('ok')
'''

_HEALTH = r'''import http.client,json,pathlib,ssl
try:
 token=pathlib.Path('/etc/momso-private/transport-token').read_text(encoding='utf-8').strip()
 context=ssl.create_default_context(cafile='/etc/momso-private/certificate.pem');context.check_hostname=False
 conn=http.client.HTTPSConnection('127.0.0.1',8766,context=context,timeout=10)
 conn.request('GET','/health',headers={'Authorization':'Bearer '+token})
 response=conn.getresponse();data=response.read(4097)
 value=json.loads(data) if len(data)<=4096 else None
 print('ready' if response.status==200 and isinstance(value,dict) and value.get('state')=='ready' else 'unready')
 conn.close()
except Exception: print('unready')
'''

_ACTIVITY = r'''import http.client,json,pathlib,re,ssl
try:
 token=pathlib.Path('/etc/momso-private/transport-token').read_text(encoding='utf-8').strip()
 context=ssl.create_default_context(cafile='/etc/momso-private/certificate.pem');context.check_hostname=False
 conn=http.client.HTTPSConnection('127.0.0.1',8766,context=context,timeout=10)
 conn.request('GET','/activity',headers={'Authorization':'Bearer '+token})
 response=conn.getresponse();data=response.read(65537)
 value=json.loads(data) if response.status==200 and len(data)<=65536 else None
 keys=('shutdownReady','busy','maintenance','activeTurns','pendingApprovals','pendingInputs','activeWrites','githubLogins','providerLogin','backupStatus','backupReason','shutdownReason')
 safe={k:value[k] for k in keys if isinstance(value,dict) and k in value and (type(value[k]) in (bool,int) or value[k] is None or (isinstance(value[k],str) and re.fullmatch(r'[a-z0-9_]{1,80}',value[k])))}
 for k in ('githubBackup','objectBackup'):
  item=value.get(k) if isinstance(value,dict) else None
  if isinstance(item,dict): safe[k]={n:item[n] for n in ('state','reason') if n in item and (item[n] is None or (isinstance(item[n],str) and re.fullmatch(r'[a-z0-9_]{1,80}',item[n])))}
 print(json.dumps(safe))
 conn.close()
except Exception: print('{}')
'''


def _sudo_python(provider, code):
    encoded = base64.b64encode(code.encode('utf-8')).decode('ascii')
    wrapper = ("import base64,subprocess; r=subprocess.run(['sudo','python3','-c',"
               f"base64.b64decode({encoded!r}).decode()],capture_output=True,timeout=60,check=False); "
               "print(r.stdout.decode('utf-8') if r.returncode==0 else '{}')")
    return provider.ssh(wrapper).decode('utf-8').strip()


def _instance(provider):
    _validate_target(provider)
    vm = provider.run('compute', 'instances', 'describe', provider.name, f'--zone={ZONE}', parse=True)
    if vm.get('name') != provider.name or vm.get('labels', {}).get('momso-install-id') != provider.install_id:
        raise ValueError('ownership_unconfirmed')
    return vm.get('status')


def prepare_idle(provider):
    if _instance(provider) != 'RUNNING':
        raise ValueError('vm_not_running')
    if _sudo_python(provider, _OVERRIDE) != 'ok':
        raise ValueError('idle_override_failed')


def health_ready(provider):
    try:
        return _instance(provider) == 'RUNNING' and _sudo_python(provider, _HEALTH) == 'ready'
    except Exception:
        return False


def idle_diagnostics(provider):
    try:
        value = json.loads(_sudo_python(provider, _ACTIVITY))
        return value if isinstance(value, dict) else {}
    except Exception:
        return {}


def gateway_url(provider):
    _validate_target(provider)
    service = provider.run('run', 'services', 'describe', 'momso-any-' + provider.install_id, f'--region={REGION}', parse=True)
    url = service.get('status', {}).get('url', '')
    parsed = urllib.parse.urlsplit(url)
    if parsed.scheme != 'https' or not parsed.hostname or not parsed.hostname.endswith('.run.app') or parsed.netloc != parsed.hostname or parsed.path or parsed.query or parsed.fragment:
        raise ValueError('invalid_gateway')
    return url


def gateway_start_operations(provider):
    """Read Compute operations without exposing account or operation output."""
    _validate_target(provider)
    items = provider.run('compute', 'operations', 'list', '--filter=operationType=start', parse=True)
    if not isinstance(items, list):
        raise ValueError('operations_unavailable')
    target = f'/zones/{ZONE}/instances/{provider.name}'
    actor = f'ma-{provider.install_id}@{provider.project}.iam.gserviceaccount.com'
    found = set()
    for item in items:
        if not isinstance(item, dict):
            raise ValueError('operations_unavailable')
        if (item.get('operationType') == 'start' and item.get('status') == 'DONE'
                and not item.get('error') and item.get('user') == actor
                and isinstance(item.get('targetLink'), str) and item['targetLink'].endswith(target)
                and isinstance(item.get('name'), str)):
            found.add(item['name'])
    return found


def public_observation(url, path, *, open_url=None):
    """Return status and content type only, never cookies, URLs with tokens or bodies."""
    if path not in ('/', '/__momso/ready'):
        raise ValueError('invalid_path')
    if open_url is None:
        class NoRedirect(urllib.request.HTTPRedirectHandler):
            def redirect_request(self, *args, **kwargs):
                return None
        open_url = urllib.request.build_opener(NoRedirect()).open
    headers = {'Cache-Control': 'no-store'}
    if path == '/':
        headers.update({'Sec-Fetch-Mode': 'navigate', 'Sec-Fetch-Dest': 'document'})
    request = urllib.request.Request(url + path, headers=headers)
    try:
        response = open_url(request, timeout=20)
    except urllib.error.HTTPError as error:
        response = error
    try:
        status = response.status
        kind = response.headers.get('Content-Type', '').split(';', 1)[0].lower()
        if kind not in ('text/html', 'application/json', 'text/plain'):
            kind = 'other'
        return {'status': status, 'kind': kind}
    finally:
        response.close()


def _notify(message):
    print(message, flush=True)


def probe_idle_wake(provider, *, clock=time.monotonic, sleep=time.sleep,
                    notify=_notify, prepare=prepare_idle, instance=_instance,
                    get_gateway=gateway_url, observe=public_observation,
                    ready=health_ready, diagnostics=idle_diagnostics,
                    start_operations=gateway_start_operations,
                    stop_timeout=900, wake_timeout=900):
    """Wait for genuine idle shutdown and owner-initiated wake; never force start."""
    _validate_target(provider)
    result = {'status': 'failed', 'stage': 'gateway_lookup', 'stopSeconds': None,
              'wakeSeconds': None, 'public': None, 'navigation': None,
              'gatewayWake': 'unverified', 'ownerResponse': 'unobserved'}
    try:
        url = get_gateway(provider)
        result['stage'] = 'idle_override'
        prepare(provider)
        result['stage'] = 'waiting_for_idle_stop'
        notify('시험 VM의 유휴 종료를 기다립니다 (최대 15분).')
        start = clock()
        while clock() - start < stop_timeout:
            if instance(provider) == 'TERMINATED':
                result['stopSeconds'] = round(clock() - start, 1)
                break
            sleep(5)
        else:
            result['stopSeconds'] = round(clock() - start, 1)
            result['activity'] = diagnostics(provider)
            return result
        result['stage'] = 'public_gateway'
        result['public'] = observe(url, '/__momso/ready')
        result['navigation'] = observe(url, '/')
        if result['public'] != {'status': 200, 'kind': 'application/json'} or result['navigation'] != {'status': 200, 'kind': 'text/html'}:
            return result
        result['stage'] = 'wake_operation_baseline'
        previous_operations = start_operations(provider)
        # Public readiness only reports gateway health and never wakes the VM.
        # An unauthenticated navigation likewise only presents login.
        notify('휴대폰에서 이 시험 주소를 열고 직접 로그인해 주세요 (최대 15분): ' + url)
        result['stage'] = 'waiting_for_owner_login_and_wake'
        start = clock()
        while clock() - start < wake_timeout:
            if instance(provider) == 'RUNNING':
                result['stage'] = 'waiting_for_health'
                if ready is not None and ready(provider):
                    result['wakeSeconds'] = round(clock() - start, 1)
                    result['stage'] = 'gateway_wake_verification'
                    if not (start_operations(provider) - previous_operations):
                        sleep(5)
                        continue
                    result['gatewayWake'] = 'verified'
                    result['status'] = 'passed'
                    result['stage'] = 'complete'
                    return result
            sleep(5)
        result['wakeSeconds'] = round(clock() - start, 1)
        if result['stage'] == 'waiting_for_owner_login_and_wake' and start_operations(provider) - previous_operations:
            result['stage'] = 'waiting_for_vm_start'
            result['gatewayWake'] = 'verified'
        return result
    except Exception:
        return result
