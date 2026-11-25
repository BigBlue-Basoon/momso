"""Retry only a custom role's known propagation failure."""
import re
import time


def role_propagation_failure(stderr):
    text = stderr.decode('utf-8', errors='replace') if isinstance(stderr, bytes) else str(stderr)
    return bool(re.search(r"(?is)role\s*\([^\r\n]*?/roles/[^\r\n)]*\)\s*does not exist in the resource.s hierarchy", text))


def bind_custom_role(provider, *args, sleep=None, attempts=12):
    if sleep is None:
        sleep = time.sleep
    for attempt in range(attempts):
        try:
            return provider.run(*args)
        except Exception as error:
            if not getattr(error, 'role_propagating', False) or attempt == attempts - 1:
                raise
            sleep(5)


def reconcile_owned_role(provider, role, permissions):
    """Restore an owned deleted role and align its permissions before use."""
    rows = provider.run('iam', 'roles', 'list', '--show-deleted', parse=True)
    if not isinstance(rows, list) or any(not isinstance(row, dict) for row in rows):
        raise ValueError('role_inventory_unavailable')
    match = next((row for row in rows if row.get('name', '').endswith('/roles/' + role)), None)
    if not match:
        return
    actual = provider.run('iam', 'roles', 'describe', role, parse=True)
    if not isinstance(actual, dict) or actual.get('description') != provider.marker:
        raise ValueError('role_ownership_mismatch')
    if actual.get('deleted'):
        provider.run('iam', 'roles', 'undelete', role)
    if set(actual.get('includedPermissions', ())) != set(permissions):
        provider.run('iam', 'roles', 'update', role, '--permissions=' + ','.join(sorted(permissions)), '--stage=GA')
