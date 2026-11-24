"""Read-only project evidence; never enable APIs or close a project."""
import json
import os
from pathlib import Path
import shutil
import subprocess


def inspect_project(provider):
    unknown = {'status': 'unverified', 'observedTypes': [], 'enabledServices': [], 'installerServices': [], 'unattributedServices': [],
               'checks': {'assets': 'unverified', 'services': 'unverified'},
               'foreignEvidence': [], 'limitations': ['unsupported_asset_types', 'asset_inventory_delay', 'service_enablement_provenance_unknown', 'iam_and_external_billing_unverified']}
    assets = services = None
    try:
        assets = provider.run('asset', 'search-all-resources', f'--scope=projects/{provider.project}', parse=True)
        if not isinstance(assets, list): assets = None
    except Exception:
        pass
    try:
        value = provider.run('services', 'list', '--enabled', parse=True)
        if isinstance(value, list):
            services = [(item.get('config') or {}).get('name') or item.get('name', '').rsplit('/', 1)[-1] for item in value]
    except Exception:
        pass
    node = shutil.which('node')
    if not node:
        return unknown
    try:
        payload = json.dumps({'projectId': provider.project, 'installationId': provider.install_id,
                              'assets': assets, 'services': services}).encode('utf-8')
        flags = getattr(subprocess, 'CREATE_NO_WINDOW', 0) if os.name == 'nt' else 0
        child = subprocess.run([node, str(Path(__file__).with_name('momso_anywhere_project_exclusivity_cli.mjs'))],
                               input=payload, stdout=subprocess.PIPE, stderr=subprocess.DEVNULL, timeout=60,
                               check=False, creationflags=flags)
        if child.returncode or len(child.stdout) > 1000000:
            return unknown
        result = json.loads(child.stdout)
        if not isinstance(result, dict) or result.get('status') not in ('not_exclusive', 'unverified'):
            return unknown
        return result
    except (OSError, ValueError, subprocess.SubprocessError):
        return unknown
