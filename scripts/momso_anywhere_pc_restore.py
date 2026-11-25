"""Prepare a new customer VM for the signed pre-start restore worker."""
import base64
import json
import re
import subprocess
import time

from momso_server_install import InstallError, REGION, ZONE, save_state
from momso_anywhere_restore_bundle import restore_files


def existing_owned_vm_can_upgrade(provider, configured):
    """Adopt no VM for restore; permit upgrade only with the original owned gateway."""
    instances = provider.run('compute', 'instances', 'list', parse=True)
    if not isinstance(instances, list): raise InstallError('restore_assets_unverified')
    matches = [item for item in instances if item.get('labels', {}).get('momso-install-id') == provider.install_id]
    if not matches: return False
    if len(matches) != 1 or matches[0].get('name') != provider.name:
        raise InstallError('restore_vm_exists')
    import momso_anywhere_install
    account, role, service = momso_anywhere_install.GoogleAnywhere(provider)._objects()
    if not account or not role or not service:
        raise InstallError('restore_vm_exists')
    current = provider.run('run', 'services', 'describe', 'momso-any-' + provider.install_id[:12], f'--region={REGION}', parse=True)
    url = current.get('status', {}).get('url')
    if not isinstance(url, str) or url != configured.get('url'):
        raise InstallError('restore_vm_exists')
    return True


def prepare_restored_vm(provider, artifact, digest, bundle, state_path, state):
    """Restore only a VM created by this recorded attempt, before starting services."""
    instances = provider.run('compute', 'instances', 'list', parse=True)
    if not isinstance(instances, list):
        raise InstallError('restore_assets_unverified')
    matches = [item for item in instances if item.get('labels', {}).get('momso-install-id') == provider.install_id]
    pending = state.get('restore_pending')
    if matches:
        if (len(matches) != 1 or not isinstance(pending, dict)
                or str(matches[0].get('id', '')) != pending.get('vmId')
                or matches[0].get('name') != provider.name):
            raise InstallError('restore_vm_exists')
    else:
        if pending and pending.get('vmId'):
            raise InstallError('restore_vm_exists')
        provider.prepare_network()
        provider.prepare_vm()
        vm = provider.run('compute', 'instances', 'describe', provider.name, f'--zone={ZONE}', parse=True)
        vm_id = str(vm.get('id', ''))
        if not vm_id.isdecimal() or vm.get('labels', {}).get('momso-install-id') != provider.install_id:
            raise InstallError('restore_assets_unverified')
        pending = {'vmId': vm_id}
        state['restore_pending'] = pending
        save_state(state_path, state)
    if not pending.get('result'):
        quiet = "import subprocess; names=('momso-server.service','momso-update.service','momso-idle-stop.service','momso-backup.service','momso-backup.timer'); assert all(subprocess.run(['systemctl','is-active','--quiet',n],stdout=subprocess.DEVNULL,stderr=subprocess.DEVNULL).returncode in (3,4) for n in names)"
        for attempt in range(12):
            try:
                provider.ssh(quiet)
                break
            except (InstallError, subprocess.TimeoutExpired):
                if attempt == 11:
                    raise InstallError('restore_failed') from None
                time.sleep(5)
        provider.install(artifact, digest, start_server=False)
        secret, backup = prepare_identity(provider)
        import momso_anywhere_vm_services as services
        services.install_scripts(provider, secret, False, backup, signed_files=restore_files(bundle, include_vault=True), start_services=False)
        pending['result'] = run_restore(provider, bundle, secret, backup)
        save_state(state_path, state)
    else:
        code = "import json; from pathlib import Path; marker=Path('/etc/momso-private/prestart-restore-complete'); report=Path('/var/lib/momso/.momso-prestart-restore.json'); assert not marker.is_symlink() and marker.read_text()==" + repr(provider.install_id) + "; assert not report.is_symlink(); value=json.loads(report.read_text()); assert value.get('ok') is True; print(json.dumps({k:value.get(k) for k in ('restored','restoredBackupId','restoredAt')}))"
        wrapper = "import base64,subprocess; subprocess.run(['sudo','python3','-c',base64.b64decode(" + repr(base64.b64encode(code.encode()).decode()) + ").decode()],check=True)"
        try:
            if json.loads(provider.ssh(wrapper)) != pending['result']:
                raise InstallError('restore_failed')
        except (ValueError, TypeError, subprocess.SubprocessError) as error:
            raise InstallError('restore_failed') from error
    return pending['result']


def prepare_identity(provider):
    import momso_anywhere_vm_services as services
    from momso_anywhere_backup_provision import GoogleBackupProvisioner

    provider.run('services', 'enable', 'secretmanager.googleapis.com', 'storage.googleapis.com', 'cloudkms.googleapis.com')
    email = services.identity(provider)
    backup = GoogleBackupProvisioner(provider, email).provision(preserved_only=True)
    secret, initial = services.secret(provider, email, preserved_only=True)
    if initial:
        raise InstallError('restore_assets_incomplete')
    vm = provider.run('compute', 'instances', 'describe', provider.name, f'--zone={ZONE}', parse=True)
    if vm.get('labels', {}).get('momso-install-id') != provider.install_id:
        raise InstallError('resource_ownership')
    accounts = vm.get('serviceAccounts', [])
    if accounts and (len(accounts) != 1 or accounts[0].get('email') != email or services.SCOPE not in accounts[0].get('scopes', [])):
        raise InstallError('resource_ownership')
    if not accounts:
        from momso_anywhere_pc_vm import vm_restart
        with vm_restart(provider):
            provider.run('compute', 'instances', 'stop', provider.name, f'--zone={ZONE}')
            provider.run('compute', 'instances', 'set-service-account', provider.name, f'--zone={ZONE}', f'--service-account={email}', f'--scopes={services.SCOPE}')
            provider.run('compute', 'instances', 'start', provider.name, f'--zone={ZONE}')
    return secret, backup


def run_restore(provider, bundle, secret, backup):
    if not re.fullmatch(r'[a-f0-9]{12}(?:[a-f0-9]{12})?', provider.install_id):
        raise InstallError('restore_assets_unverified')
    if (not re.fullmatch(r'[a-z][a-z0-9-]{4,28}[a-z0-9]', provider.project)
            or secret != 'momso-vault-' + provider.install_id[:12]
            or backup.get('bucket') != 'momso-backup-' + provider.install_id[:12]
            or backup.get('key') != 'projects/' + provider.project + '/locations/asia-northeast3/keyRings/momso-backup-' + provider.install_id[:12] + '/cryptoKeys/backup'):
        raise InstallError('restore_assets_unverified')
    contents = restore_files(bundle, include_vault=True)
    payload = {'files': {name: base64.b64encode(data).decode('ascii') for name, data in contents.items()},
               'id': provider.install_id, 'project': provider.project, 'secret': secret,
               'bucket': backup['bucket'], 'key': backup['key']}
    script = r'''import base64,json,os,pathlib,subprocess,sys
data=json.load(sys.stdin);root=pathlib.Path('/opt/momso-restore');private=pathlib.Path('/etc/momso-private')
private.mkdir(mode=0o700,exist_ok=True)
if private.is_symlink():raise RuntimeError('restore_failed')
root.mkdir(mode=0o755,exist_ok=True)
if root.is_symlink():raise RuntimeError('restore_failed')
owner=root/'owner'
if owner.exists():
 if owner.is_symlink() or owner.read_text()!=data['id']:raise RuntimeError('restore_failed')
else:
 if any(root.iterdir()):raise RuntimeError('restore_failed')
 owner.write_text(data['id'])
for name,value in data['files'].items():
 if '/' in name or not name.endswith('.py'):raise RuntimeError('restore_failed')
 path=root/name;value=base64.b64decode(value,validate=True)
 if os.path.lexists(path):
  if path.is_symlink() or not path.is_file() or path.read_bytes()!=value:raise RuntimeError('restore_failed')
 else:
  with path.open('xb') as f:f.write(value);f.flush();os.fsync(f.fileno())
  os.chmod(path,0o644)
def run(*args):subprocess.run(args,check=True,stdout=subprocess.DEVNULL,stderr=subprocess.DEVNULL)
missing=[]
if subprocess.run(['/usr/bin/python3','-c','import cryptography'],stdout=subprocess.DEVNULL,stderr=subprocess.DEVNULL).returncode:missing.append('python3-cryptography')
if not __import__('shutil').which('age-keygen'):missing.append('age')
if missing:
 os.environ['DEBIAN_FRONTEND']='noninteractive';run('apt-get','update','-qq');run('apt-get','install','-y','-qq',*missing)
os.makedirs('/run/momso-vault',mode=0o700,exist_ok=True)
try:run('/usr/bin/python3',str(root/'vault-provision.py'),'--project',data['project'],'--secret',data['secret'],'--vault-dir','/var/lib/momso')
except subprocess.CalledProcessError:
 print(json.dumps({'ok':False,'code':'restore_failed','detail':'key_unavailable'}));sys.exit(0)
marker=private/'prestart-restore-complete';report=pathlib.Path('/var/lib/momso/.momso-prestart-restore.json')
if os.path.lexists(marker):
 if marker.is_symlink() or marker.read_text()!=data['id']:raise RuntimeError('restore_failed')
else:
 completion=root/'restore-complete.py'
 source="import os\np="+repr(str(marker))+"\nwith open(p,'x') as f:f.write("+repr(data['id'])+");f.flush();os.fsync(f.fileno())\nfd=os.open(os.path.dirname(p),os.O_RDONLY|os.O_DIRECTORY);os.fsync(fd);os.close(fd)\n"
 if completion.exists():
  if completion.is_symlink() or completion.read_text()!=source:raise RuntimeError('restore_failed')
 else:completion.write_text(source)
 unit=pathlib.Path('/etc/systemd/system/momso-restore.service')
 text='[Unit]\nDescription=momso pre-start restore\n[Service]\nType=oneshot\nUser=momso\nLoadCredential=momso-vault-identity:/run/momso-vault/momso-vault-identity\nExecStart=/usr/bin/python3 '+str(root)+'/momso_anywhere_prestart_restore.py --installation-id '+data['id']+' --data-dir /var/lib/momso --bucket '+data['bucket']+' --kms-key '+data['key']+' --vault-identity /run/credentials/momso-restore.service/momso-vault-identity --report '+str(report)+'\nExecStartPost=+/usr/bin/python3 '+str(completion)+'\n'
 if unit.exists():
  if unit.is_symlink() or unit.read_text()!=text:raise RuntimeError('restore_failed')
 else:unit.write_text(text)
 run('systemctl','daemon-reload')
 if report.exists() and not report.is_symlink():
  try:
   if json.loads(report.read_text()).get('ok') is True:raise RuntimeError('restore_interrupted')
  except ValueError:pass
 status=subprocess.run(['systemctl','start','momso-restore.service'],stdout=subprocess.DEVNULL,stderr=subprocess.DEVNULL)
 if status.returncode:
  try:value=json.loads(report.read_text())
  except Exception:value={'ok':False,'code':'restore_failed','detail':'backup_unreadable'}
  print(json.dumps({'ok':False,'code':'restore_failed','detail':value.get('detail','backup_unreadable')}));sys.exit(0)
if not report.is_file() or report.is_symlink():raise RuntimeError('restore_failed')
value=json.loads(report.read_text())
if value.get('ok') is not True or not isinstance(value.get('restored'),bool):raise RuntimeError('restore_failed')
print(json.dumps({k:value.get(k) for k in ('restored','restoredBackupId','restoredAt')}))
'''
    wrapper = "import base64,subprocess,sys; subprocess.run(['sudo','python3','-c',base64.b64decode(" + repr(base64.b64encode(script.encode()).decode()) + ").decode()],input=sys.stdin.buffer.read(),check=True)"
    for attempt in range(12):
        try:
            output = provider.ssh(wrapper, json.dumps(payload).encode())
            break
        except (InstallError, subprocess.TimeoutExpired):
            if attempt == 11:
                raise InstallError('restore_failed') from None
            time.sleep(5)
    try:
        result = json.loads(output)
    except (ValueError, TypeError):
        raise InstallError('restore_failed') from None
    if result.get('ok') is False:
        detail = result.get('detail', 'backup_unreadable')
        if not re.fullmatch(r'[a-z_]{1,64}', str(detail)):
            detail = 'backup_unreadable'
        raise InstallError('restore_failed: ' + detail)
    if not isinstance(result.get('restored'), bool):
        raise InstallError('restore_failed')
    return result
