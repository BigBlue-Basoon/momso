"""Invoke the shared backup cleanup without exposing cloud credentials."""
import json
import os
from pathlib import Path
import shutil
import subprocess
from momso_server_install import InstallError


def discard_backup(provider, previous=None):
    unknown = {'mode': 'delete_all', 'resources': {
        'bucket': {'status': 'unverified', 'reason': 'cleanup_unavailable'},
        'kms': {'status': 'unverified', 'reason': 'cleanup_unavailable'}},
        'billable': [], 'billingZero': None, 'estimatedZeroAt': None}
    if isinstance(previous, dict) and isinstance(previous.get('billable'), list):
        unknown['billable'] = [item for item in previous['billable'][:100]
                               if isinstance(item, dict) and item.get('resource') in ('bucket', 'kms')
                               and (item.get('endsAt') is None or isinstance(item.get('endsAt'), str))]
    node = shutil.which('node')
    if not node:
        return unknown
    try:
        token = provider.run('auth', 'print-access-token').decode('utf-8').strip()
        if not token or any(c.isspace() for c in token):
            return unknown
        payload = json.dumps({'token': token, 'projectId': provider.project, 'installationId': provider.install_id, 'previous': previous}).encode('utf-8')
        flags = getattr(subprocess, 'CREATE_NO_WINDOW', 0) if os.name == 'nt' else 0
        result = subprocess.run([node, str(Path(__file__).with_suffix('.mjs'))], input=payload,
                                stdout=subprocess.PIPE, stderr=subprocess.DEVNULL, timeout=900,
                                check=False, creationflags=flags)
        if result.returncode or len(result.stdout) > 131072:
            return unknown
        value = json.loads(result.stdout)
        if not isinstance(value, dict) or value.get('mode') != 'delete_all' or value.get('billingZero') is not None or not isinstance(value.get('resources'), dict) or not isinstance(value.get('billable'), list):
            return unknown
        return value
    except (InstallError, OSError, ValueError, UnicodeError, subprocess.SubprocessError):
        return unknown
