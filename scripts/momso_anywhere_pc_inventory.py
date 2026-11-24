"""Read-only, conservative recheck of resources owned by one installation."""
from momso_server_install import REGION, ZONE


def inspect_destroyed(provider):
    try:
        install_id = provider.install_id
        marker = provider.marker
        specs = {
            'vm': [(('compute', 'instances', 'list', f'--zones={ZONE}'), {provider.name}, 'label')],
            'disk': [(('compute', 'disks', 'list', f'--zones={ZONE}'), {provider.name}, 'label')],
            'gateway': [(('run', 'services', 'list', f'--region={REGION}'), {'momso-any-' + install_id, 'momso-wake-' + install_id}, 'label')],
            'network': [(('compute', 'networks', 'list'), {provider.network}, 'marker'),
                        (('compute', 'networks', 'subnets', 'list', f'--regions={REGION}'), {provider.subnet}, 'marker'),
                        (('compute', 'routers', 'list', f'--regions={REGION}'), {provider.router}, 'marker'),
                        (('compute', 'firewall-rules', 'list'), {provider.firewall, provider.name + '-private'}, 'marker')],
            'secrets': [(('secrets', 'list'), {'momso-transport-' + install_id, 'momso-session-' + install_id}, 'label')],
            'iam': [(('iam', 'service-accounts', 'list'), {'ma-' + install_id, 'mw-' + install_id, 'mv-' + install_id}, 'marker'),
                    (('iam', 'roles', 'list'), {'momsoAnywhere' + install_id, 'momsoWake' + install_id, 'momsoStop' + install_id, 'momsoUpdate' + install_id}, 'marker')],
        }
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
    return result
