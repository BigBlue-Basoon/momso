"""Disposable PC restore rehearsal; never exposes VM credentials or marker values."""
import base64
import datetime
import hashlib
import json
import os
from pathlib import Path
import secrets
import subprocess
import sys
import tempfile

import momso_anywhere_rehearsal_s117a as s117
from momso_anywhere_restore_pc import inspect_restore
from momso_server_install import GoogleProvider, initialize_state


def make_adapter(args):
    import momso_release_keys
    import momso_release_manifest
    root = Path(args.bundle)
    server, web, key = root / 'momso-server', root / 'web.tar.gz', Path(args.public_key)
    momso_release_keys.validate(args.channel, key)
    signed = momso_release_manifest.release(args.release_manifest_url, key, channel=args.channel)
    archive = momso_release_manifest.download(signed['bundleUrl'], 536870912)
    momso_release_manifest.verify_inputs(archive, signed['bundleSha256'], server, web)
    from momso_anywhere_restore_bundle import restore_files
    restore_files(archive, include_vault=True)
    directory = Path(tempfile.mkdtemp(prefix='momso-s147-'))
    state_path = directory / 'state.json'
    state = initialize_state(state_path, args.account, args.project)
    provider = GoogleProvider(args.account, args.project, state['id'], directory / ('.momso-ssh-' + state['id']) / 'ssh')
    provider.protected_installation_ids = ({value[:12] for value in args.protect_installation_id}
        if args.protect_installation_id else s117.discover_protected_installations(provider))
    s117._validate_target(provider)
    installer = str(Path(__file__).with_name('momso_server_install.py'))
    common = ['--account', args.account, '--project', args.project, '--state', str(state_path)]
    install = [sys.executable, installer, 'anywhere', *common, '--artifact', str(server), '--sha256', s117.sha256_file(server),
               '--web-archive', str(web), '--web-sha256', s117.sha256_file(web), '--google-client-id', args.google_client_id,
               '--owner-sub', args.owner_sub, '--login-origin', args.login_origin, '--release-manifest-url', args.release_manifest_url,
               '--release-public-key', str(key), '--release-channel', args.channel]
    return RealAdapter(provider, state_path, install, [sys.executable, installer, 'destroy', *common]), state['id'], provider.protected_installation_ids


class RealAdapter:
    def __init__(self, provider, state_path, install, destroy, *, run=subprocess.run):
        self.provider, self.state_path, self.install_command, self.destroy_command = provider, state_path, install, destroy
        self.run = run
        self.marker = secrets.token_hex(32)
        self.last_deletion = None

    def _installer(self, command):
        result = self.run(command, stdout=subprocess.PIPE, stderr=subprocess.PIPE, check=False)
        return result.returncode == 0, result.stdout

    def _vm(self, code):
        encoded = base64.b64encode(code.encode('utf-8')).decode('ascii')
        wrapper = "import base64,subprocess; subprocess.run(['sudo','-u','momso','python3','-c',base64.b64decode(" + repr(encoded) + ").decode()],check=True)"
        return self.provider.ssh(wrapper).decode('utf-8').strip()

    def preflight(self, identity):
        return identity == self.provider.install_id and s117.preflight_new_id(self.provider)

    def install(self, identity):
        return identity == self.provider.install_id and self._installer(self.install_command)[0]

    def write_marker(self, identity):
        if identity != self.provider.install_id: raise ValueError('invalid_scope')
        code = "import hashlib,sqlite3; db=sqlite3.connect('/var/lib/momso/class-student-private.sqlite3'); db.execute('CREATE TABLE IF NOT EXISTS s147_rehearsal (value TEXT NOT NULL)'); db.execute('DELETE FROM s147_rehearsal'); db.execute('INSERT INTO s147_rehearsal VALUES (?)',(" + repr(self.marker) + ",)); db.commit(); db.close(); print(hashlib.sha256(" + repr(self.marker.encode()) + ").hexdigest())"
        return self._vm(code)

    def pause(self, identity):
        return identity == self.provider.install_id and self._installer(self.destroy_command)[0]

    def check_paused(self, identity):
        if identity != self.provider.install_id: raise ValueError('invalid_scope')
        inventory = s117.inventory_basic_resources(self.provider)
        gateway = self.provider.run('run', 'services', 'list', '--platform=managed', f'--region={s117.REGION}', parse=True)
        if not isinstance(gateway, list): raise ValueError('inventory_unavailable')
        gateway_name = 'momso-any-' + identity[:12]
        gateway_absent = all((row.get('metadata', {}).get('name') or row.get('name', '')).rsplit('/', 1)[-1] != gateway_name for row in gateway)
        return {'vmAbsent': inventory['vm']['complete'] and not inventory['vm']['items'],
                'gatewayAbsent': gateway_absent, 'backupPresent': inspect_restore(self.provider)['restoreAvailable'] is True}

    def reinstall(self, identity):
        if identity != self.provider.install_id: return False
        succeeded, _ = self._installer(self.install_command)
        if not succeeded: return False
        state = json.loads(self.state_path.read_text(encoding='utf-8'))
        return state.get('restore_result', {}).get('restored') is True

    def read_marker(self, identity):
        if identity != self.provider.install_id: raise ValueError('invalid_scope')
        code = "import hashlib,sqlite3; db=sqlite3.connect('file:/var/lib/momso/class-student-private.sqlite3?mode=ro',uri=True); value=db.execute('SELECT value FROM s147_rehearsal').fetchone()[0]; db.close(); print(hashlib.sha256(value.encode()).hexdigest())"
        return self._vm(code)

    def cleanup(self, identity):
        if identity != self.provider.install_id: return False
        succeeded, output = self._installer([*self.destroy_command, '--discard-data'])
        for line in output.decode('utf-8', errors='replace').splitlines():
            try: value = json.loads(line)
            except ValueError: continue
            if isinstance(value, dict) and 'outcome' in value: self.last_deletion = value
        return succeeded and self.last_deletion is not None and self.last_deletion.get('outcome') in ('deleted', 'deleted_unverified')

    def inventory(self, identity):
        if identity != self.provider.install_id: raise ValueError('invalid_scope')
        groups, kms = s117.collect_inventory(self.provider)
        remaining = [name for name, group in groups.items() if group.get('items')]
        unknown = [name for name, group in groups.items() if group.get('complete') is not True]
        def settled(item):
            if item.get('billing') == 'not_billable': return True
            if item.get('billing') != 'billable' or not isinstance(item.get('until'), str): return False
            try: return datetime.datetime.fromisoformat(item['until'].replace('Z', '+00:00')).tzinfo is not None
            except ValueError: return False
        scheduled = bool(kms) and all(settled(item) for item in kms)
        retained_kms = groups.get('kms', {}).get('items', []) if scheduled else []
        if scheduled and 'kms' in remaining: remaining.remove('kms')
        outcome = (self.last_deletion or {}).get('outcome')
        if unknown and outcome == 'deleted': outcome = 'deleted_unverified'
        return {'complete': not unknown and not remaining, 'remaining': remaining,
                'unverified': unknown, 'kmsVersions': kms, 'retainedKms': retained_kms,
                'deletionOutcome': outcome}
