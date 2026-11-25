"""PC-only disposable install rehearsal; never run it against an existing installation."""
from __future__ import annotations

import base64
import argparse
import hashlib
import json
import re
import subprocess
import sys
import tempfile
import urllib.error
import urllib.parse
import urllib.request
from pathlib import Path

from momso_server_install import REGION, GoogleProvider, initialize_state


def _validate_target(provider):
    value = getattr(provider, "install_id", None)
    protected = getattr(provider, 'protected_installation_ids', ())
    if not isinstance(value, str) or not re.fullmatch(r"[a-f0-9]{12}", value) or value in protected:
        raise ValueError("invalid_installation_id")


def discover_protected_installations(provider):
    """Read existing installations; failure or malformed listings stop rehearsal."""
    protected = set()
    rows = provider.run('compute', 'instances', 'list', parse=True)
    if not isinstance(rows, list):
        raise ValueError('protected_inventory_unavailable')
    for row in rows:
        if not isinstance(row, dict) or not isinstance(row.get('name'), str):
            raise ValueError('protected_inventory_unavailable')
        match = re.fullmatch(r'momso-(?:any-)?([a-f0-9]{12})', row['name'])
        if match:
            protected.add(match.group(1))
    token = provider.run('auth', 'print-access-token').decode('utf-8').strip()
    if not token or re.search(r'\s', token):
        raise ValueError('protected_inventory_unavailable')
    class NoRedirect(urllib.request.HTTPRedirectHandler):
        def redirect_request(self, req, fp, code, msg, headers, newurl): return None
    opener = urllib.request.build_opener(urllib.request.ProxyHandler({}), NoRedirect())
    page = ''
    while True:
        url = 'https://run.googleapis.com/apis/serving.knative.dev/v1/namespaces/' + provider.project + '/services'
        if page:
            url += '?' + urllib.parse.urlencode({'continue': page})
        request = urllib.request.Request(url, headers={'Authorization': 'Bearer ' + token})
        with opener.open(request, timeout=30) as response:
            data = json.load(response)
        if not isinstance(data, dict) or data.get('unreachable'):
            raise ValueError('protected_inventory_unavailable')
        rows = data.get('items', [])
        if not isinstance(rows, list):
            raise ValueError('protected_inventory_unavailable')
        for row in rows:
            if not isinstance(row, dict) or not isinstance(row.get('metadata', {}), dict):
                raise ValueError('protected_inventory_unavailable')
            name = row.get('metadata', {}).get('name')
            if not isinstance(name, str):
                raise ValueError('protected_inventory_unavailable')
            match = re.fullmatch(r'momso-(?:any-)?([a-f0-9]{12})', name)
            if match:
                protected.add(match.group(1))
        page = data.get('metadata', {}).get('continue', '')
        if not isinstance(page, str):
            raise ValueError('protected_inventory_unavailable')
        if not page:
            break
    return protected


def gateway_evidence(provider, expected_image, *, open_url=None):
    """Check the actual receiving Cloud Run revision and a public HTTPS probe."""
    _validate_target(provider)
    if not re.fullmatch(r"[a-zA-Z0-9._/-]+@sha256:[a-f0-9]{64}", expected_image or ""):
        raise ValueError("invalid_signed_image")
    service = "momso-any-" + provider.install_id
    evidence = {"traffic_digest": "unverified", "public_https": "unverified"}
    try:
        result = provider.run("run", "services", "describe", service, f"--region={REGION}", parse=True)
        status = result["status"]
    except Exception:
        return evidence
    try:
        traffic = [item for item in status["traffic"] if item.get("percent", 0)]
        if (len(traffic) == 1 and type(traffic[0].get("percent")) is int
                and traffic[0]["percent"] == 100
                and re.fullmatch(r"[a-z0-9-]+", traffic[0].get("revisionName", ""))):
            revision = provider.run("run", "revisions", "describe", traffic[0]["revisionName"], f"--region={REGION}", parse=True)
            state = revision["status"]
            ready = any(item.get("type") == "Ready" and item.get("status") == "True" for item in state.get("conditions", []))
            actual = state.get("imageDigest", "")
            if ready and isinstance(actual, str) and actual.split("@")[-1] == expected_image.split("@")[-1]:
                evidence["traffic_digest"] = "passed"
    except Exception:
        pass
    try:
        url = urllib.parse.urlsplit(status["url"])
        if (url.scheme == "https" and url.hostname and url.hostname.endswith(".run.app")
                and url.netloc == url.hostname and not url.path and not url.query and not url.fragment):
            if open_url is None:
                class NoRedirect(urllib.request.HTTPRedirectHandler):
                    def redirect_request(self, *args, **kwargs):
                        return None
                open_url = urllib.request.build_opener(NoRedirect()).open
            request = urllib.request.Request(status["url"] + "/__momso/ready", headers={"Cache-Control": "no-store"})
            with open_url(request, timeout=20) as response:
                if response.status == 200:
                    evidence["public_https"] = "passed"
    except Exception:
        # Do not print gcloud output, request headers, tokens or remote errors.
        pass
    return evidence


_VM_PROBE = r'''import http.client,json,ssl,subprocess
units=['momso-server','momso-private-proxy','momso-idle-stop','momso-update']
result={'services':{},'update_status':'unverified'}
for unit in units:
 try:
  p=subprocess.run(['systemctl','is-active',unit+'.service'],capture_output=True,timeout=10,check=False)
  result['services'][unit]='active' if p.returncode==0 and p.stdout.strip()==b'active' else 'unverified'
 except Exception: result['services'][unit]='unverified'
try:
 token=open('/etc/momso-private/transport-token',encoding='utf-8').read().strip()
 context=ssl.create_default_context(cafile='/etc/momso-private/certificate.pem');context.check_hostname=False
 conn=http.client.HTTPSConnection('127.0.0.1',8766,context=context,timeout=10)
 conn.request('GET','/update/status',headers={'Authorization':'Bearer '+token})
 response=conn.getresponse();body=response.read(4097)
 value=json.loads(body) if len(body)<=4096 else None
 if response.status==200 and isinstance(value,dict) and value.get('state') in ('checking','up_to_date','downloading','waiting_for_idle','backing_up','installing','updating_gateway','completed'):result['update_status']='passed'
 conn.close()
except Exception: pass
print(json.dumps(result,separators=(',',':')))
'''


def vm_evidence(provider):
    """Return only unit/status verdicts; token remains on the VM."""
    _validate_target(provider)
    encoded = base64.b64encode(_VM_PROBE.encode()).decode('ascii')
    wrapper = (
        "import base64,subprocess; r=subprocess.run(['sudo','python3','-c',"
        f"base64.b64decode({encoded!r}).decode()],capture_output=True,timeout=60,check=False); "
        "print(r.stdout.decode('utf-8') if r.returncode==0 else '{}')"
    )
    try:
        result = json.loads(provider.ssh(wrapper))
        units = ('momso-server', 'momso-private-proxy', 'momso-idle-stop', 'momso-update')
        return {
            "vm_services": "passed" if all(result.get('services', {}).get(unit) == 'active' for unit in units) else "unverified",
            "update_status": "passed" if result.get('update_status') == 'passed' else "unverified",
        }
    except Exception:
        return {"vm_services": "unverified", "update_status": "unverified"}


def inventory_basic_resources(provider):
    """List project-wide resources, retaining an unknown result on any failure.

    This is deliberately not the complete inventory: nested router NATs, IAM
    bindings, KMS versions and storage retention need separate inspection.
    """
    _validate_target(provider)
    commands = {
        'vm': ('compute', 'instances', 'list'),
        'disk': ('compute', 'disks', 'list'),
        'network': ('compute', 'networks', 'list'),
        'subnet': ('compute', 'networks', 'subnets', 'list'),
        'firewall': ('compute', 'firewall-rules', 'list'),
        'router': ('compute', 'routers', 'list'),
        'addresses': ('compute', 'addresses', 'list', f'--regions={REGION}'),
        'gateway': ('run', 'services', 'list', '--platform=managed', f'--region={REGION}'),
        'accounts': ('iam', 'service-accounts', 'list'),
        'roles': ('iam', 'roles', 'list'),
        'secrets': ('secrets', 'list'),
        'bucket': ('storage', 'buckets', 'list'),
    }
    result = {key: {'complete': False, 'items': []}
              for key in (*commands, 'nat', 'iam', 'kms')}
    for kind, command in commands.items():
        try:
            rows = provider.run(*command, parse=True)
            if not isinstance(rows, list):
                continue
            found = []
            for row in rows:
                if not isinstance(row, dict):
                    raise ValueError('invalid_resource')
                metadata = row.get('metadata', {})
                if not isinstance(metadata, dict):
                    raise ValueError('invalid_resource')
                names = [value for value in (row.get('name'), row.get('email'), metadata.get('name')) if isinstance(value, str)]
                labels = row.get('labels', metadata.get('labels', {}))
                if not names or not isinstance(labels, dict):
                    raise ValueError('invalid_resource')
                matches = (('momso-backup-' + provider.install_id) in names if kind == 'bucket'
                           else any(provider.install_id in value for value in names) or labels.get('momso-install-id') == provider.install_id)
                if kind == 'addresses':
                    subnet = row.get('subnetwork', '')
                    if not isinstance(subnet, str): raise ValueError('invalid_resource')
                    matches = matches or subnet.endswith('/subnetworks/momso-' + provider.install_id + '-sub')
                if matches:
                    found.append(next((value for value in names if provider.install_id in value), names[0]))
            # The installer creates its Cloud Run services in REGION only.
            result[kind] = {'complete': True, 'items': found}
        except Exception as error:
            # An incomplete listing is never represented as an empty complete list.
            if kind == 'bucket' and getattr(error, 'http_status', None) == 403:
                result[kind]['reason'] = 'bucket_access_denied'
            continue
    try:
        from momso_anywhere_network_cleanup import installation_nats
        result['nat'] = {'complete': True, 'items': installation_nats(provider)}
    except Exception:
        result['nat']['reason'] = 'nat_inventory_failed'
    try:
        from momso_anywhere_iam_pc import inspect_installation_bindings
        result['iam'] = {'complete': True, 'items': inspect_installation_bindings(provider)}
    except Exception as error:
        result['iam']['items'] = getattr(error, 'found', [])
        result['iam']['reason'] = ('soft_deleted_bucket_policy_unavailable'
                                   if str(error) == 'soft_deleted_bucket_policy_unavailable' else 'iam_inventory_failed')
    try:
        if not result['bucket']['complete']:
            if result['bucket'].get('reason') == 'bucket_access_denied':
                raise PermissionError('bucket_access_denied')
            raise ValueError('bucket_inventory_failed')
        expected = 'momso-backup-' + provider.install_id
        try:
            bucket = provider.run('storage', 'buckets', 'describe', 'gs://' + expected, parse=True)
        except Exception as error:
            if getattr(error, 'http_status', None) == 403:
                raise PermissionError('bucket_access_denied') from None
            if getattr(error, 'http_status', None) != 404 or result['bucket']['items']:
                raise ValueError('bucket_inventory_failed') from None
            bucket = None
        if bucket is not None:
            if not isinstance(bucket, dict) or bucket.get('name') != expected:
                raise ValueError('bucket_inventory_failed')
            if bucket.get('labels', {}).get('momso-install-id') != provider.install_id:
                raise ValueError('bucket_ownership_unverified')
            if expected not in result['bucket']['items']:
                raise ValueError('bucket_inventory_inconsistent')
        from momso_anywhere_iam_pc import soft_deleted_buckets
        soft = soft_deleted_buckets(provider)
        result['bucket']['softDeleted'] = soft
        result['bucket']['items'].extend(item['name'] + ' (soft-deleted)' for item in soft)
    except Exception as error:
        result['bucket']['complete'] = False
        result['bucket']['reason'] = ('bucket_access_denied' if isinstance(error, PermissionError)
                                      or getattr(error, 'http_status', None) == 403 else 'bucket_inventory_failed')
    return result


def preflight_new_id(provider):
    """Reject collisions and inaccessible scope before creating anything."""
    groups = inventory_basic_resources(provider)
    for kind, group in groups.items():
        if kind in ('nat', 'iam', 'kms', 'gateway'):
            continue
        if not group['complete'] or group['items']:
            return False
    # This installer uses exactly REGION for its Cloud Run and KMS resources.
    try:
        services = provider.run('run', 'services', 'list', '--platform=managed', f'--region={REGION}', parse=True)
        keys = inventory_regional_backup_keys(provider)
        if not isinstance(services, list) or not keys['complete'] or keys['rings']:
            return False
        for item in services:
            if not isinstance(item, dict) or not isinstance(item.get('metadata', {}), dict):
                return False
            metadata = item.get('metadata', {})
            name = item.get('name') or metadata.get('name')
            labels = item.get('labels', metadata.get('labels', {}))
            if not isinstance(name, str) or not isinstance(labels, dict):
                return False
            if provider.install_id in name or labels.get('momso-install-id') == provider.install_id:
                return False
        return True
    except Exception:
        return False


def sha256_file(path):
    digest = hashlib.sha256()
    with path.open('rb') as source:
        for block in iter(lambda: source.read(1024 * 1024), b''):
            digest.update(block)
    return digest.hexdigest()


def schedule_owned_backup_keys(provider):
    """Schedule destruction only for the freshly created, labelled backup key."""
    _validate_target(provider)
    ring = 'momso-backup-' + provider.install_id
    rings = provider.run('kms', 'keyrings', 'list', f'--location={REGION}', parse=True)
    if not isinstance(rings, list):
        raise ValueError('kms_listing_unavailable')
    expected_ring = f'projects/{provider.project}/locations/{REGION}/keyRings/{ring}'
    if not any(item.get('name') == expected_ring for item in rings):
        return 'keys_absent'
    keys = provider.run('kms', 'keys', 'list', f'--location={REGION}', f'--keyring={ring}', parse=True)
    if not isinstance(keys, list):
        raise ValueError('kms_listing_unavailable')
    expected_key = expected_ring + '/cryptoKeys/backup'
    if any(not isinstance(key, dict) or key.get('name') != expected_key or key.get('labels', {}).get('momso-install-id') != provider.install_id for key in keys):
        raise ValueError('kms_ownership_unconfirmed')
    pending = []
    if keys:
        versions = provider.run('kms', 'keys', 'versions', 'list', f'--location={REGION}', f'--keyring={ring}', '--key=backup', parse=True)
        if not isinstance(versions, list):
            raise ValueError('kms_listing_unavailable')
        for version in versions:
            if not isinstance(version, dict):
                raise ValueError('kms_listing_unavailable')
            name, state = version.get('name'), version.get('state')
            prefix = expected_key + '/cryptoKeyVersions/'
            if not isinstance(name, str) or not name.startswith(prefix) or not re.fullmatch(r'[0-9]+', name[len(prefix):]) or state not in ('ENABLED', 'DISABLED', 'DESTROY_SCHEDULED', 'DESTROYED'):
                raise ValueError('kms_ownership_unconfirmed')
            if state in ('ENABLED', 'DISABLED'):
                pending.append(name[len(prefix):])
    for version in pending:
        provider.run('kms', 'keys', 'versions', 'destroy', version, f'--location={REGION}', f'--keyring={ring}', '--key=backup')
    return 'keys_scheduled'


def collect_inventory(provider):
    groups = inventory_basic_resources(provider)
    keys = inventory_regional_backup_keys(provider)
    # The installer only creates the expected keyring in REGION. The billing
    # verdict is separate from whether resource names remain.
    if keys['complete']:
        groups['kms'] = {'complete': True, 'items': keys['rings']}
    versions = []
    for version in keys['versions']:
        state = version['state']
        billing = 'billable' if state in ('ENABLED', 'DISABLED', 'DESTROY_SCHEDULED') else 'not_billable' if state == 'DESTROYED' else 'unknown'
        versions.append({'name': version['name'], 'billing': billing, 'until': version['destroyTime']})
    return groups, versions


_INSTALLER_CODES = frozenset(('gateway_image_permission', 'gateway_image_invalid', 'gateway_image_mismatch',
    'gateway_traffic_mismatch', 'gateway_probe_failed', 'gateway_rollback_failed', 'gateway_not_ready',
    'gateway_operation_failed', 'gateway_response_invalid', 'gateway_address_invalid', 'gateway_ownership',
    'gateway_policy', 'rollback_failed', 'backup_not_connected', 'backup_freshness_unverified', 'vm_unreachable', 'vm_state_unknown', 'vm_start_failed', 'vm_busy',
    'cleanup_pending', 'cleanup_incomplete', 'delete_failed', 'proxy_not_running', 'proxy_cleanup_failed'))


def installer_result(process, stage):
    """Extract only known machine codes, never arbitrary subprocess output."""
    reason = 'installer_failed'
    try:
        for line in process.stdout.decode('utf-8', errors='replace').splitlines():
            try: value = json.loads(line)
            except ValueError: continue
            if isinstance(value, dict):
                for key in ('code', 'reason'):
                    if value.get(key) in _INSTALLER_CODES: reason = value[key]
    except (UnicodeError, AttributeError, TypeError):
        pass
    result = {'ok': process.returncode == 0, 'stage': stage, 'reason': None if process.returncode == 0 else reason}
    if process.returncode:
        # Installer-owned diagnostics only; never echo arbitrary tool output or secrets.
        stderr = getattr(process, 'stderr', b'') or b''
        lines = stderr.decode('utf-8', errors='replace').splitlines()
        safe = [line for line in lines if line.startswith('실패: ') and len(line) <= 300 and not re.search(r'(?i)token|secret|password|credential|bearer|https?://|[A-Za-z0-9+/=_-]{48,}', line)]
        if safe: result['stderrTail'] = safe[-1]
    return result


def run_rehearsal(provider, install, verify, destroy, backup_cleanup, inventory, idle=None, network_retry=None):
    """Callbacks permit fake-provider tests; cleanup always runs after start."""
    _validate_target(provider)
    result = {'install': 'failed', 'checks': {}, 'destroy': 'failed', 'backup_cleanup': 'unknown', 'inventory': {}, 'kmsVersions': []}
    stage = 'install'
    try:
        installed = install()
        if isinstance(installed, dict):
            result['installFailure'] = None if installed.get('ok') else {key: installed[key] for key in ('stage', 'reason', 'stderrTail') if key in installed}
            installed = installed.get('ok') is True
        if installed:
            result['install'] = 'passed'
            stage = 'verify'
            result['checks'] = verify()
            failures = [name for name, value in result['checks'].items() if value != 'passed']
            if failures: result['checkFailures'] = sorted(failures)
            if idle is not None:
                required = ('traffic_digest', 'public_https', 'vm_services', 'update_status')
                result['idleWake'] = idle(provider) if all(result['checks'].get(k) == 'passed' for k in required) else {'status': 'failed', 'stage': 'initial_checks'}
    except Exception:
        if stage == 'verify': result['checkFailures'] = ['verification_unavailable']
        else: result['installFailure'] = {'stage': stage, 'reason': 'rehearsal_operation_failed'}
    finally:
        try:
            deletion = destroy()
            if isinstance(deletion, dict):
                result['deletion'] = deletion
                result['destroy'] = 'passed' if deletion.get('outcome') in ('deleted', 'deleted_unverified') else 'failed'
                if result['destroy'] == 'failed':
                    code = deletion.get('reason')
                    result['destroyFailure'] = code if code in _INSTALLER_CODES or code == 'serverless_address_release_pending' else 'delete_failed'
            else:
                result['destroy'] = 'passed' if deletion else 'failed'
        except Exception:
            result['destroyFailure'] = 'delete_failed'
        try:
            result['backup_cleanup'] = backup_cleanup()
        except Exception:
            pass
        try:
            result['inventory'], result['kmsVersions'] = inventory()
            remaining = [name for name, group in result['inventory'].items()
                         if name != 'kms' and isinstance(group, dict) and group.get('items')]
            if remaining:
                result['destroy'] = 'failed'
                result['remainingKinds'] = sorted(remaining)
            deletion = result.get('deletion')
            groups = result['inventory']
            if isinstance(deletion, dict) and deletion.get('outcome') == 'deleted_unverified':
                unresolved = []
                resources = deletion.setdefault('resources', {})
                for name in deletion.get('unverified', []):
                    group = groups.get('iam' if name == 'iam_bindings' else name, {})
                    if group.get('complete') is not True:
                        unresolved.append(name)
                    else:
                        retained = bool(group.get('items'))
                        resources[name] = {'status': 'retained' if retained else 'deleted',
                                           'reason': 'cleanup_pending' if retained else None}
                deletion['unverified'] = unresolved
                if not unresolved:
                    for kind in remaining:
                        resources[kind] = {'status': 'retained', 'reason': 'cleanup_pending'}
                    retained = any(item.get('status') == 'retained' for item in resources.values() if isinstance(item, dict))
                    deletion.update(outcome='remaining' if retained else 'deleted', complete=not retained,
                                    status='pending' if retained else 'complete')
                    result['destroy'] = 'failed' if retained else 'passed'
            other = ('vm', 'disk', 'firewall', 'router', 'nat', 'gateway', 'accounts', 'roles', 'secrets', 'iam', 'bucket', 'kms')
            if (isinstance(deletion, dict) and deletion.get('outcome') == 'remaining'
                    and remaining and set(remaining).issubset({'network', 'subnet', 'addresses'})
                    and all(groups.get(kind, {}).get('complete') is True for kind in ('network', 'subnet', 'addresses'))
                    and all(groups.get(kind, {}).get('complete') is True and not groups[kind].get('items') for kind in other)
                    and not result['kmsVersions']):
                from momso_anywhere_network_cleanup import pending_serverless
                expected = (['networks subnets:' + provider.subnet] if 'subnet' in remaining else []) + (['networks:' + provider.network] if 'network' in remaining else [])
                pending = pending_serverless(provider, expected)
                if pending and pending['addressCount'] == len(groups['addresses'].get('items', [])):
                    retry = network_retry() if network_retry is not None else {}
                    deletion.update(pending)
                    deletion.update(retry)
                    result['destroyFailure'] = pending['reason']
        except Exception:
            pass
    return result


def main(argv=None):
    parser = argparse.ArgumentParser(description='momso 시험 설치·검사·정리')
    for name in ('account', 'project', 'release-manifest-url', 'bundle', 'public-key', 'channel', 'google-client-id', 'owner-sub', 'login-origin'):
        parser.add_argument('--' + name, required=True)
    parser.add_argument('--check-idle', action='store_true', help='시험 VM의 유휴 종료와 사용자 로그인 후 깨우기를 확인')
    parser.add_argument('--protect-installation-id', action='append', default=[])
    args = parser.parse_args(argv)
    if (not re.fullmatch(r'[A-Za-z0-9._%+@-]+', args.account) or
            not re.fullmatch(r'[a-z][a-z0-9-]{4,28}[a-z0-9]', args.project) or
            args.channel not in ('production', 'development') or
            not re.fullmatch(r'[A-Za-z0-9._-]+\.apps\.googleusercontent\.com', args.google_client_id) or
            not re.fullmatch(r'[0-9]{1,40}', args.owner_sub) or
            not re.fullmatch(r'https://[a-z0-9][a-z0-9.-]*', args.login_origin)):
        parser.error('인자를 확인하세요')
    bundle, key = Path(args.bundle), Path(args.public_key)
    server, web = bundle / 'momso-server', bundle / 'web.tar.gz'
    try:
        import momso_release_keys, momso_release_manifest
        momso_release_keys.validate(args.channel, key)
        signed = momso_release_manifest.release(args.release_manifest_url, key, channel=args.channel)
        archive = momso_release_manifest.download(signed['bundleUrl'], 536870912)
        momso_release_manifest.verify_inputs(archive, signed['bundleSha256'], server, web)
        server_hash, web_hash = sha256_file(server), sha256_file(web)
        directory = Path(tempfile.mkdtemp(prefix='momso-s117a-'))
        state_path = directory / 'state.json'
        state = initialize_state(state_path, args.account, args.project)
        provider = GoogleProvider(args.account, args.project, state['id'], directory / ('ssh-' + state['id']))
        if any(not re.fullmatch(r'[a-f0-9]{12}(?:[a-f0-9]{12})?', value) for value in args.protect_installation_id):
            raise ValueError('invalid_installation_id')
        provider.protected_installation_ids = ({value[:12] for value in args.protect_installation_id}
            if args.protect_installation_id else discover_protected_installations(provider))
        _validate_target(provider)
        if not preflight_new_id(provider):
            print('설치 전 충돌 검사 또는 권한 확인 실패. 설치하지 않았습니다.')
            return 1
        installer = str(Path(__file__).with_name('momso_server_install.py'))
        common = ['--account', args.account, '--project', args.project, '--state', str(state_path)]
        command = [sys.executable, installer, 'anywhere', *common, '--artifact', str(server), '--sha256', server_hash,
                   '--web-archive', str(web), '--web-sha256', web_hash, '--google-client-id', args.google_client_id,
                   '--owner-sub', args.owner_sub, '--login-origin', args.login_origin, '--release-manifest-url', args.release_manifest_url,
                   '--release-public-key', str(key), '--release-channel', args.channel]
        def run(command):
            process = subprocess.run(command, stdout=subprocess.PIPE, stderr=subprocess.PIPE, check=False)
            try:
                saved = json.loads(state_path.read_text(encoding='utf-8'))
                steps = saved.get('steps', {})
                stage = next((name for name in ('network', 'vm', 'install', 'health') if steps.get(name) != 'ok'), 'after_health')
            except (OSError, ValueError, AttributeError):
                stage = 'unknown'
            return installer_result(process, stage)
        def run_destroy():
            process = subprocess.run([sys.executable, installer, 'destroy', *common, '--discard-data'], stdout=subprocess.PIPE, stderr=subprocess.DEVNULL, check=False)
            try:
                value = json.loads(process.stdout.decode('utf-8'))
                return value if isinstance(value, dict) else False
            except (ValueError, UnicodeError):
                return False
        # SSH uses the key directory the existing installer created.
        provider.key_file = directory / ('.momso-ssh-' + state['id']) / 'ssh'
        def verify():
            return {**gateway_evidence(provider, signed['gatewayImage']), **vm_evidence(provider)}
        def cleanup_backup():
            from momso_anywhere_backup_provision import GoogleBackupProvisioner
            bucket_result = GoogleBackupProvisioner(provider).destroy(delete_backup=True)
            key_result = schedule_owned_backup_keys(provider)
            return bucket_result + ',' + key_result
        if args.check_idle:
            from momso_anywhere_rehearsal_idle_s117b import probe_idle_wake
        from momso_server_install import network_retry_command
        retry_args = argparse.Namespace(account=args.account, project=args.project, state=state_path, discard_data=True)
        result = run_rehearsal(provider, lambda: run(command), verify,
                               run_destroy,
                               cleanup_backup, lambda: collect_inventory(provider),
                               idle=probe_idle_wake if args.check_idle else None,
                               network_retry=lambda: network_retry_command(retry_args))
        print('시험 설치 ID:', state['id'])
        print('설치:', result['install'], '확인:', json.dumps(result['checks'], ensure_ascii=False), '삭제:', result['destroy'])
        if result.get('installFailure'): print('설치 실패 단계·코드:', json.dumps(result['installFailure'], ensure_ascii=False))
        if result.get('checkFailures'): print('확인 실패 항목:', ', '.join(result['checkFailures']))
        if result.get('destroyFailure'): print('삭제 실패 코드:', result['destroyFailure'])
        deletion = result.get('deletion', {})
        if deletion.get('reason') == 'serverless_address_release_pending':
            print('Cloud Run 내부 주소 회수 대기: 해당 내부 주소와 일반 서브넷은 과금 없음')
            if deletion.get('retryCommand'): print('남은 네트워크 재확인:', deletion['retryCommand'])
        if result.get('deletion', {}).get('outcome') == 'deleted_unverified':
            print('삭제 후 미확인:', ', '.join(result['deletion'].get('unverified', [])))
        if args.check_idle:
            print('유휴 종료·깨우기:', json.dumps(result.get('idleWake', {'status': 'failed', 'stage': 'installation'}), ensure_ascii=False))
        unknown, remaining = [], []
        for kind in ('vm', 'disk', 'network', 'subnet', 'firewall', 'router', 'nat', 'addresses', 'gateway', 'accounts', 'roles', 'secrets', 'iam', 'bucket', 'kms'):
            group = result['inventory'].get(kind, {})
            items = group.get('items', [])
            status = '남음' if items else '없음' if group.get('complete') is True else '미확인'
            if status == '미확인':
                print(kind + ':', status, '(' + group.get('reason', '조회 실패') + ')')
            else:
                print(kind + ':', status, str(len(items)) + '개', ', '.join(items), '(일부 미확인)' if items and group.get('complete') is not True else '')
            if group.get('complete') is not True:
                unknown.append(kind)
            if items:
                remaining.append(kind)
        for version in result['kmsVersions']:
            label = {'billable': '과금 대상', 'not_billable': '과금 안 됨', 'unknown': '과금 미확인'}[version['billing']]
            print('KMS:', version['name'], label, '파기 시각:', version['until'] or '없음 또는 미확인')
        chargeable = [v for v in result['kmsVersions'] if v['billing'] == 'billable']
        if chargeable:
            print('아직 과금 대상: KMS 키 버전', len(chargeable), '개 (위 파기 시각 참조)')
        billable_kinds = ('vm', 'disk', 'bucket', 'gateway', 'nat') + (() if deletion.get('reason') == 'serverless_address_release_pending' else ('addresses',))
        for kind in billable_kinds:
            if kind in remaining:
                print('과금 가능 자원 남음:', kind)
        if unknown or any(v['billing'] == 'unknown' for v in result['kmsVersions']) or any(kind in remaining for kind in billable_kinds):
            print('과금되는 것 0 확인: 미확인; 남은 자원 또는 확인하지 못한 종류를 확인하세요.')
        elif not chargeable:
            print('과금되는 것: 확인한 범위에서 0')
        print('시험용 백업 정리:', result['backup_cleanup'])
        print('재확인용 상태 기록:', state_path)
        unexpected_remaining = [kind for kind in remaining if kind != 'kms']
        kms_scheduled = bool(result['kmsVersions']) and all(v['billing'] == 'not_billable' or (v['billing'] == 'billable' and v.get('until')) for v in result['kmsVersions'])
        return 0 if result['install'] == 'passed' and result['destroy'] == 'passed' and not unknown and not unexpected_remaining and ('kms' not in remaining or kms_scheduled) and result['backup_cleanup'] in ('backup_absent,keys_absent', 'backup_absent,keys_scheduled', 'backup_deleted,keys_absent', 'backup_deleted,keys_scheduled') and all(result['checks'].get(k) == 'passed' for k in ('traffic_digest', 'public_https', 'vm_services', 'update_status')) and (not args.check_idle or result.get('idleWake', {}).get('status') == 'passed') else 1
    except Exception:
        print('실패: 입력 또는 서명·접근 권한을 확인하세요.', file=sys.stderr)
        return 1


def inventory_regional_backup_keys(provider):
    """Inspect key versions in the region used by the PC backup provisioner.

    No conclusion about other regions or physical keyring deletion is made.
    """
    _validate_target(provider)
    result = {'complete': False, 'region': REGION, 'rings': [], 'versions': []}
    try:
        rings = provider.run('kms', 'keyrings', 'list', f'--location={REGION}', parse=True)
        if not isinstance(rings, list):
            return result
        for ring in rings:
            name = ring.get('name')
            if not isinstance(name, str):
                return result
            short = name.rsplit('/', 1)[-1]
            if provider.install_id not in short:
                continue
            result['rings'].append(name)
            keys = provider.run('kms', 'keys', 'list', f'--location={REGION}', f'--keyring={short}', parse=True)
            if not isinstance(keys, list):
                return result
            for key in keys:
                key_name = key.get('name')
                if not isinstance(key_name, str):
                    return result
                key_short = key_name.rsplit('/', 1)[-1]
                versions = provider.run('kms', 'keys', 'versions', 'list', f'--location={REGION}',
                                        f'--keyring={short}', f'--key={key_short}', parse=True)
                if not isinstance(versions, list):
                    return result
                for version in versions:
                    version_name, state = version.get('name'), version.get('state')
                    if not isinstance(version_name, str) or not isinstance(state, str):
                        return result
                    result['versions'].append({'name': version_name, 'state': state,
                                               'destroyTime': version.get('destroyTime')})
        result['complete'] = True
    except Exception:
        pass
    return result


if __name__ == '__main__':
    sys.exit(main())
