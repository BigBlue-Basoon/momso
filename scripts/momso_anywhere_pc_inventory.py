"""Read-only, conservative recheck of resources owned by one installation."""
from momso_server_install import REGION, ZONE
from momso_anywhere_vm_resources import names as vm_resource_names


def confirmed_billing_zero(groups, versions, resources, billable):
    """Only claim zero when every installation resource was read and absent."""
    required = ('vm', 'disk', 'network', 'subnet', 'firewall', 'router', 'nat', 'addresses',
                'gateway', 'accounts', 'roles', 'secrets', 'iam', 'bucket', 'kms')
    if not isinstance(groups, dict) or not isinstance(versions, list) or not isinstance(resources, dict): return False
    if versions or billable: return False
    return (all(groups.get(name, {}).get('complete') is True and not groups[name].get('items') for name in required)
            and bool(resources) and all(isinstance(item, dict) and item.get('status') == 'deleted' for item in resources.values()))


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
    try:
        from momso_anywhere_network_cleanup import installation_nats
        nats = installation_nats(provider)
        result['nat'] = {'status': 'retained' if nats else 'deleted',
                         'reason': 'cleanup_pending' if nats else None, 'count': len(nats)}
    except Exception:
        result['nat'] = {'status': 'unverified', 'reason': 'nat_inventory_failed'}
    try:
        from momso_anywhere_iam_pc import inspect_installation_bindings
        bindings = inspect_installation_bindings(provider)
        result['iam_bindings'] = {'status': 'retained' if bindings else 'deleted',
                                  'reason': 'cleanup_pending' if bindings else None,
                                  'count': len(bindings)}
    except Exception as error:
        reason = 'soft_deleted_bucket_policy_unavailable' if str(error) == 'soft_deleted_bucket_policy_unavailable' else 'iam_inventory_failed'
        found = getattr(error, 'found', [])
        if found:
            result['iam_bindings'] = {'status': 'retained', 'reason': 'cleanup_pending', 'count': len(found)}
            result['iam_bindings_inventory'] = {'status': 'unverified', 'reason': reason}
        else:
            result['iam_bindings'] = {'status': 'unverified', 'reason': reason}
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
