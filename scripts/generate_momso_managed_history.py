"""Generate VM installer legacy fingerprints from paths in the integrated Git history."""
import argparse
import hashlib
import json
from pathlib import Path
import subprocess

ROOT = Path(__file__).resolve().parent.parent
OUTPUT = ROOT / 'scripts/momso_managed_history.json'
FILES = {
    'vault-provision.py': 'apps/alpha/desktop/src-tauri/momso-vault-provision/provision.py',
    'idle-stop.py': 'apps/alpha/desktop/src-tauri/momso-idle-stop/idle_stop.py',
    **{name: 'scripts/' + name for name in ('momso_anywhere_backup.py', 'momso_anywhere_backup_agent.py', 'momso_anywhere_backup_google.py', 'momso_anywhere_vault_identity.py')},
}
WORKER = {name: 'apps/alpha/desktop/src-tauri/momso-server-update/' + name for name in (
    'update.py', 'release_manifest.py', 'server_admission.py', 'systemd_host.py', 'data_snapshots.py', 'cloud_run_gateway.py',
    'github_releases.py', 'progress_status.py', 'layout_migration.py', 'install_worker.py', 'worker.py', 'ensure_gcloud.py')}
PROXY = {name: 'scripts/' + name for name in ('momso_mobile_proxy.py', 'momso_anywhere_vm_proxy.py')}


def generate(root=ROOT):
    shallow = subprocess.run(['git', 'rev-parse', '--is-shallow-repository'], cwd=root, check=True, capture_output=True).stdout.strip()
    if shallow != b'false':
        raise ValueError('managed history requires a complete Git history')
    groups = {'files': FILES, 'worker': WORKER, 'proxy': PROXY}
    paths = {path for group in groups.values() for path in group.values()}
    objects = {path: set() for path in paths}
    # Query each path separately: Git otherwise emits a shared blob only once
    # and assigns it to just one of the selected paths.
    for expected in sorted(paths):
        result = subprocess.run(['git', 'rev-list', '--objects', '--full-history', 'HEAD', '--', expected], cwd=root, check=True, capture_output=True)
        for line in result.stdout.decode('utf-8').splitlines():
            oid, separator, path = line.partition(' ')
            if separator and path == expected and len(oid) == 40:
                objects[path].add(oid)
    cache = {}
    for oid in sorted({oid for values in objects.values() for oid in values}):
        value = subprocess.run(['git', 'cat-file', 'blob', oid], cwd=root, check=True, capture_output=True).stdout
        cache[oid] = hashlib.sha256(value).hexdigest()
    return {'version': 1, **{group: {name: sorted({cache[oid] for oid in objects[path]}) for name, path in mapping.items()} for group, mapping in groups.items()}}


def main():
    parser = argparse.ArgumentParser()
    parser.add_argument('--check', action='store_true')
    args = parser.parse_args()
    data = json.dumps(generate(), indent=2, sort_keys=True) + '\n'
    if args.check:
        if not OUTPUT.is_file() or OUTPUT.read_text(encoding='utf-8') != data:
            raise SystemExit('managed history is stale; regenerate and commit it')
    else:
        OUTPUT.write_text(data, encoding='utf-8', newline='\n')
    print('managed history: OK')


if __name__ == '__main__':
    main()
