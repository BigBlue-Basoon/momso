"""Resize an owned VM while its existing maintenance gate remains closed."""
from __future__ import annotations

import argparse
import base64
import json
import os
from pathlib import Path
import re
import sys
import time
import urllib.error
import urllib.parse
import urllib.request

from momso_server_install import GoogleProvider, REGION, ZONE, is_link_or_reparse, key_directory

MACHINES = {"e2-small", "e2-medium"}


class ResizeError(Exception):
    def __init__(self, code, *, rollback=None, gate=None):
        super().__init__(code)
        self.code, self.rollback, self.gate = code, rollback, gate


def load_state(path, account, project):
    path = Path(path)
    if is_link_or_reparse(path) or not path.is_file():
        raise ResizeError("resize_failed")
    try:
        state = json.loads(path.read_text(encoding="utf-8"))
    except (OSError, ValueError):
        raise ResizeError("resize_failed") from None
    if (state.get("version") != 1 or state.get("account") != account or state.get("project") != project
            or not re.fullmatch(r"[0-9a-f]{12}", state.get("id", ""))):
        raise ResizeError("resize_failed")
    return state


def gate(provider, action):
    if action not in ("acquire", "release", "status"):
        raise ValueError("invalid gate action")
    code = r'''import fcntl,json,os,time,urllib.request
env=dict(line.strip().split('=',1) for line in open('/etc/momso-server.env') if '=' in line and not line.lstrip().startswith('#'))
token=env['MOMSO_SERVER_TOKEN']
def call(path,method='GET'):
 req=urllib.request.Request('http://127.0.0.1:8765'+path,headers={'Authorization':'Bearer '+token},method=method,data=b'' if method=='POST' else None)
 with urllib.request.urlopen(req,timeout=5) as res:return json.load(res)
if ACTION=='acquire':
 data=env['MOMSO_SERVER_DATA_DIR']; lifecycle=os.path.join(data,'lifecycle')
 if os.path.islink(data) or os.path.islink(lifecycle):raise RuntimeError('lifecycle_invalid')
 os.makedirs(lifecycle,mode=0o700,exist_ok=True)
 owner=os.stat(data);os.chown(lifecycle,owner.st_uid,owner.st_gid)
 lock=os.open(os.path.join(lifecycle,'lifecycle.lock'),os.O_RDWR|os.O_CREAT|os.O_NOFOLLOW,0o600)
 os.fchown(lock,owner.st_uid,owner.st_gid);fcntl.flock(lock,fcntl.LOCK_EX|fcntl.LOCK_NB)
 if os.path.exists(os.path.join(lifecycle,'idle-stop.pending')):raise RuntimeError('lifecycle_busy')
 before=call('/activity')
 if before.get('maintenance') is not False:raise RuntimeError('maintenance_busy')
 if call('/maintenance','POST').get('maintenance') is not True:raise RuntimeError('maintenance_unconfirmed')
 try:
  deadline=time.monotonic()+120
  while True:
   status=call('/activity')
   if status.get('maintenanceReady') is True:break
   if time.monotonic()>=deadline:raise RuntimeError('maintenance_not_ready')
   time.sleep(0.25)
 except Exception:
  try:call('/maintenance/release','POST')
  except Exception:pass
  raise
elif ACTION=='release':
 call('/maintenance/release','POST')
 if call('/activity').get('maintenance') is not False:raise RuntimeError('maintenance_release_unconfirmed')
else:
 value=call('/activity').get('maintenance')
 if not isinstance(value,bool):raise RuntimeError('maintenance_status_unavailable')
 print('gate_closed' if value else 'gate_open')
print('gate_ok')
'''.replace('ACTION', repr(action))
    wrapper = "import base64,subprocess;subprocess.run(['sudo','python3','-c',base64.b64decode(" + repr(base64.b64encode(code.encode()).decode()) + ").decode()],check=True)"
    result = provider.ssh(wrapper)
    if b'gate_ok' not in result:
        raise ResizeError("resize_failed")
    if action == 'status':
        if b'gate_closed' in result:
            return True
        if b'gate_open' in result:
            return False
        raise ResizeError("resize_failed")


class NoRedirect(urllib.request.HTTPRedirectHandler):
    def redirect_request(self, req, fp, code, msg, headers, newurl):
        return None


def gateway_probe(url, session):
    parsed = urllib.parse.urlsplit(url)
    if (parsed.scheme != "https" or not parsed.hostname or parsed.username or parsed.password
            or parsed.path not in ("", "/") or parsed.query or parsed.fragment):
        return False
    if not session or not re.fullmatch(r"[A-Za-z0-9._~-]{1,4096}", session):
        return False
    request = urllib.request.Request(url.rstrip('/') + '/health', headers={'Cookie': '__Host-momso-session=' + session})
    try:
        opener = urllib.request.build_opener(NoRedirect())
        with opener.open(request, timeout=15) as response:
            return response.status == 200 and json.load(response).get('state') == 'ready'
    except (OSError, ValueError, urllib.error.URLError):
        return False


def verify_owned(provider, state):
    vm = provider.run('compute', 'instances', 'describe', provider.name, f'--zone={ZONE}', parse=True)
    if vm.get('labels', {}).get('momso-install-id') != provider.install_id or vm.get('status') != 'RUNNING':
        raise ResizeError('resize_failed')
    interfaces = vm.get('networkInterfaces', [])
    if not interfaces or any(interface.get('accessConfigs') or interface.get('ipv6AccessConfigs')
                             or not interface.get('network', '').endswith('/' + provider.network) for interface in interfaces):
        raise ResizeError('resize_failed')
    original = vm.get('machineType', '').rsplit('/', 1)[-1]
    if original not in MACHINES:
        raise ResizeError('resize_failed')
    from momso_anywhere_install import GoogleAnywhere
    account, role, service = GoogleAnywhere(provider)._objects()
    if not account or not role or not service:
        raise ResizeError('resize_failed')
    current = provider.run('run', 'services', 'describe', 'momso-any-' + provider.install_id, f'--region={REGION}', parse=True)
    url = state.get('anywhere', {}).get('url')
    if not isinstance(url, str) or current.get('status', {}).get('url') != url:
        raise ResizeError('resize_failed')
    return original, url


def resize(provider, state, target, probe, gate_fn=gate, sleep=time.sleep):
    if target not in MACHINES:
        raise ResizeError('resize_failed')
    try:
        original, url = verify_owned(provider, state)
        if not probe(url):
            raise ResizeError('health_not_ready')
        if original == target:
            return {'ok': True, 'machineType': original, 'changed': False}
    except ResizeError:
        raise
    except Exception:
        raise ResizeError('resize_failed') from None
    try:
        gate_fn(provider, 'acquire')
    except Exception:
        # The request may have reached the server even if its response was lost.
        raise ResizeError('resize_failed', gate='unconfirmed') from None
    error = None
    try:
        provider.run('compute', 'instances', 'stop', provider.name, f'--zone={ZONE}')
        provider.run('compute', 'instances', 'set-machine-type', provider.name, f'--machine-type={target}', f'--zone={ZONE}')
        try:
            provider.run('compute', 'instances', 'start', provider.name, f'--zone={ZONE}')
        except Exception:
            raise ResizeError('vm_start_failed') from None
        for attempt in range(24):
            if probe(url):
                break
            if attempt == 23:
                raise ResizeError('health_not_ready')
            sleep(5)
    except ResizeError as exc:
        error = exc.code
    except Exception:
        error = 'resize_failed'
    if error is None:
        try:
            gate_fn(provider, 'release')
            return {'ok': True, 'machineType': target, 'changed': True}
        except Exception:
            try:
                closed = gate_fn(provider, 'status')
            except Exception:
                raise ResizeError('resize_failed', gate='unconfirmed') from None
            if closed is False:
                return {'ok': True, 'machineType': target, 'changed': True}
            if closed is not True:
                raise ResizeError('resize_failed', gate='unconfirmed') from None
            error = 'resize_failed'
    if error:
        try:
            provider.run('compute', 'instances', 'stop', provider.name, f'--zone={ZONE}')
            provider.run('compute', 'instances', 'set-machine-type', provider.name, f'--machine-type={original}', f'--zone={ZONE}')
            provider.run('compute', 'instances', 'start', provider.name, f'--zone={ZONE}')
            restored = any(probe(url) or (sleep(5) or False) for _ in range(24))
            if not restored:
                raise RuntimeError('rollback_health_unconfirmed')
            gate_fn(provider, 'release')
        except Exception:
            raise ResizeError(error, rollback='unconfirmed', gate='retained') from None
        raise ResizeError(error, rollback='restored', gate='released')
    raise ResizeError('resize_failed', gate='unconfirmed')


def main(argv=None):
    parser = argparse.ArgumentParser()
    parser.add_argument('--state', required=True, type=Path)
    parser.add_argument('--account', required=True)
    parser.add_argument('--project', required=True)
    parser.add_argument('--machine-type', required=True, choices=sorted(MACHINES))
    parser.add_argument('--owner-session-file', type=Path)
    args = parser.parse_args(argv)
    try:
        state = load_state(args.state, args.account, args.project)
        session_file = args.owner_session_file
        if session_file is None or is_link_or_reparse(session_file) or not session_file.is_file():
            raise ResizeError('health_not_ready')
        session = session_file.read_text(encoding='utf-8').strip()
        directory = key_directory(args.state, state['id'], False)
        provider = GoogleProvider(args.account, args.project, state['id'], directory / 'ssh')
        result = resize(provider, state, args.machine_type, lambda url: gateway_probe(url, session))
    except ResizeError as error:
        result = {'ok': False, 'reason': error.code, 'rollback': error.rollback, 'maintenance': error.gate}
    except Exception:
        result = {'ok': False, 'reason': 'resize_failed'}
    print(json.dumps(result, ensure_ascii=False))
    return 0 if result['ok'] else 1


if __name__ == '__main__':
    sys.exit(main())
