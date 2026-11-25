"""Read-only, conservative recheck of resources owned by one installation."""
from momso_server_install import REGION, ZONE
from momso_anywhere_vm_resources import names as vm_resource_names


def inspect_destroyed(provider, *, discard_data=False):
    try:
        install_id = provider.install_id
        marker = provider.marker
        vm = vm_resource_names(provider)
        specs = {
            'vm': [(('compute', 'instances', 'list', f'--zones={ZONE}'), {provider.name}, 'label')],
            'disk': [(('compute', 'disks', 'list', f'--zones={ZONE}'), {provider.name}, 'label')],
            'gateway': [(('run', 'services', 'list', f'--region={REGION}'), {vm['gateway'], vm['wake']}, 'label')],
            'network': [(('compute', 'networks', 'list'), {provider.network}, 'marker'),
                        (('compute', 'networks', 'subnets', 'list', f'--regions={REGION}'), {provider.subnet}, 'marker'),
                        (('compute', 'routers', 'list', f'--regions={REGION}'), {provider.router}, 'marker'),
                        (('compute', 'firewall-rules', 'list'), {provider.firewall, provider.name + '-private'}, 'marker')],
            'secrets': [(('secrets', 'list'), {vm['transport_secret'], vm['session_secret']} | ({vm['vault_secret']} if discard_data else set()), 'label')],
            'iam': [(('iam', 'service-accounts', 'list'), {vm['gateway_account'], vm['wake_account'], vm['account'], vm['backup_account']}, 'marker')],
        }
        if discard_data:
            specs['iam'].append((('iam', 'roles', 'list'), {vm['gateway_role'], vm['wake_role'], *vm['roles']}, 'marker'))
    except AttributeError:
        return {name: {'status': 'unverified', 'reason': 'not_inventoried'} for name in ('vm', 'disk', 'gateway', 'network', 'secrets', 'iam', 'iam_bindings')}
    result = {}
    for group, checks in specs.items():
        found = False
        unknown = False
        for command, names, ownership in checks:
            try:
                rows = provider.run(*command, parse=True)
                if not isinstance(rows, list):
                    raise ValueError('invalid_inventory')
                for row in rows:
                    if not isinstance(row, dict):
                        raise ValueError('invalid_inventory')
                    metadata = row.get('metadata') or {}
                    raw = row.get('name') or metadata.get('name') or row.get('email') or ''
                    name = raw.rsplit('/', 1)[-1].split('@', 1)[0]
                    if name not in names or row.get('deleted') is True:
                        continue
                    labels = row.get('labels') or metadata.get('labels') or {}
                    owned = labels.get('momso-install-id') == install_id if ownership == 'label' else row.get('description') == marker
                    if owned:
                        found = True
                    else:
                        unknown = True
            except Exception:
                unknown = True
        status = 'retained' if found else 'unverified' if unknown else 'deleted'
        result[group] = {'status': status, 'reason': None if status == 'deleted' else 'cleanup_pending' if found else 'inventory_unavailable'}
        if found and unknown:
            result[group + '_inventory'] = {'status': 'unverified', 'reason': 'inventory_unavailable'}
    result['iam_bindings'] = {'status': 'unverified', 'reason': 'not_inventoried'}
    if not discard_data:
        try:
            rows = provider.run('iam', 'roles', 'list', parse=True)
            if not isinstance(rows, list) or any(not isinstance(item, dict) for item in rows): raise ValueError('invalid_inventory')
            expected = {vm['gateway_role'], vm['wake_role'], *vm['roles']}
            matching = [item for item in rows if item.get('name', '').rsplit('/', 1)[-1] in expected and not item.get('deleted')]
            if any(item.get('description') != marker for item in matching): raise ValueError('ownership_unverified')
            result['free_roles'] = {'status': 'retained' if matching else 'deleted', 'reason': 'backup_recovery' if matching else None, 'billing': 'free', 'names': sorted(item['name'].rsplit('/', 1)[-1] for item in matching)}
        except Exception:
            result['free_roles'] = {'status': 'unverified', 'reason': 'inventory_unavailable'}
    return result
