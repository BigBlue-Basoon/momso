"""Inspect and retry only one installation's remaining VPC resources."""
from momso_server_install import REGION, ZONE, InstallError
from momso_anywhere_vm_resources import names


def installation_nats(provider):
    routers = provider.run('compute', 'routers', 'list', f'--regions={REGION}', parse=True)
    if (not isinstance(routers, list) or any(not isinstance(row, dict) or not isinstance(row.get('name'), str)
                                             or not row['name'] for row in routers)):
        raise ValueError('nat_inventory_failed')
    expected_router = 'momso-' + provider.install_id + '-router'
    matching = [row for row in routers if row.get('name') == expected_router]
    if len(matching) > 1: raise ValueError('nat_inventory_failed')
    if not matching: return []
    router = provider.run('compute', 'routers', 'describe', expected_router, f'--region={REGION}', parse=True)
    if not isinstance(router, dict) or router.get('name') != expected_router:
        raise ValueError('nat_inventory_failed')
    nats = router.get('nats', [])
    if not isinstance(nats, list) or any(not isinstance(row, dict) or not isinstance(row.get('name'), str) for row in nats):
        raise ValueError('nat_inventory_failed')
    return [row['name'] for row in nats if row['name'] == 'momso-' + provider.install_id + '-nat']


def serverless_addresses(provider):
    try:
        rows = provider.run('compute', 'addresses', 'list', f'--regions={REGION}', parse=True)
    except Exception:
        raise ValueError('address_inventory_failed') from None
    if not isinstance(rows, list) or any(not isinstance(row, dict) for row in rows):
        raise ValueError('address_inventory_failed')
    found = []
    for row in rows:
        subnet = row.get('subnetwork', '')
        if not isinstance(subnet, str): raise ValueError('address_inventory_failed')
        if not subnet.endswith('/subnetworks/' + provider.subnet): continue
        if row.get('purpose') != 'SERVERLESS' or row.get('status') != 'RESERVED': continue
        if row.get('addressType') != 'INTERNAL' or not isinstance(row.get('name'), str):
            raise ValueError('address_inventory_failed')
        found.append(row['name'])
    return found


def pending_serverless(provider, remaining):
    expected = {'networks subnets:' + provider.subnet, 'networks:' + provider.network}
    if not remaining or any(item not in expected for item in remaining): return None
    subnets = provider.run('compute', 'networks', 'subnets', 'list', f'--regions={REGION}', parse=True)
    if (not isinstance(subnets, list) or any(not isinstance(row, dict) or not isinstance(row.get('name'), str)
                                             or not row['name'] for row in subnets)):
        raise ValueError('subnet_inventory_failed')
    matches = [row for row in subnets if row.get('name') == provider.subnet]
    if len(matches) != 1 or matches[0].get('description') != provider.marker:
        raise ValueError('subnet_inventory_failed')
    if matches[0].get('purpose', 'PRIVATE') != 'PRIVATE':
        return None
    addresses = serverless_addresses(provider)
    if not addresses: return None
    try:
        rows = provider.run('compute', 'addresses', 'list', f'--regions={REGION}', parse=True)
    except Exception:
        raise ValueError('address_inventory_failed') from None
    if not isinstance(rows, list) or any(not isinstance(row, dict) for row in rows):
        raise ValueError('address_inventory_failed')
    related = []
    for row in rows:
        name, subnet = row.get('name'), row.get('subnetwork', '')
        if not isinstance(name, str) or not isinstance(subnet, str): raise ValueError('address_inventory_failed')
        if provider.install_id in name or subnet.endswith('/subnetworks/' + provider.subnet):
            if row.get('purpose') != 'SERVERLESS' or row.get('status') != 'RESERVED' or row.get('addressType') != 'INTERNAL':
                return None
            related.append(name)
    if sorted(related) != sorted(addresses): return None
    return {'reason': 'serverless_address_release_pending', 'addressCount': len(addresses),
            'billing': 'no_charge', 'noChargeResources': ['network', 'subnet', 'reserved_internal_address'],
            'retryable': True}


def other_inventory(provider):
    """Return explicit evidence, including failed checks, for other types."""
    from momso_anywhere_rehearsal_s117a import collect_inventory
    groups, versions = collect_inventory(provider)
    other = ('vm', 'disk', 'firewall', 'router', 'nat', 'gateway', 'accounts',
             'roles', 'secrets', 'iam', 'bucket', 'kms')
    result = {}
    for kind in other:
        group = groups.get(kind, {})
        items = group.get('items', [])
        complete = group.get('complete') is True
        name = 'iam_bindings' if kind == 'iam' else kind
        result[name] = {'status': 'retained' if items else 'deleted' if complete else 'unverified',
                        'reason': 'cleanup_pending' if items else None if complete else group.get('reason', 'inventory_unavailable')}
        if items:
            result[name]['names'] = list(items)
            result[name]['count'] = len(items)
        if items and not complete:
            result[name + '_inventory'] = {'status': 'unverified', 'reason': group.get('reason', 'inventory_unavailable')}
    return result, versions


def only_network_remains(provider):
    """A network-only conclusion requires successful inventory of other types."""
    result, versions = other_inventory(provider)
    return all(item['status'] == 'deleted' for item in result.values()) and not versions


def retry_network_only(provider):
    """Refuse when a VM or gateway still exists; never delete an address."""
    owned = names(provider)
    instances = provider.run('compute', 'instances', 'list', f'--zones={ZONE}', parse=True)
    services = provider.run('run', 'services', 'list', f'--region={REGION}', parse=True)
    if not isinstance(instances, list) or not isinstance(services, list): raise ValueError('inventory_unavailable')
    if any(not isinstance(row, dict) for row in instances + services): raise ValueError('inventory_unavailable')
    if any(not isinstance(row.get('name'), str) or not row['name'] for row in instances): raise ValueError('inventory_unavailable')
    if any(row['name'].rsplit('/', 1)[-1] == provider.name for row in instances): raise ValueError('other_resources_present')
    for row in services:
        metadata = row.get('metadata') or {}
        if not isinstance(metadata, dict): raise ValueError('inventory_unavailable')
        service_name = row.get('name') or metadata.get('name')
        if not isinstance(service_name, str) or not service_name: raise ValueError('inventory_unavailable')
        if service_name.rsplit('/', 1)[-1] in (owned['gateway'], owned['wake']):
            raise ValueError('other_resources_present')
    remaining = []
    for parts, name, list_flag, delete_flag in (
        (('networks', 'subnets'), provider.subnet, f'--regions={REGION}', f'--region={REGION}'),
        (('networks',), provider.network, None, None),
    ):
        args = ('compute', *parts, 'list', *((list_flag,) if list_flag else ()))
        rows = provider.run(*args, parse=True)
        if (not isinstance(rows, list) or any(not isinstance(row, dict) or not isinstance(row.get('name'), str)
                                              or not row['name'] for row in rows)): raise ValueError('inventory_unavailable')
        matching = [row for row in rows if row.get('name') == name]
        if len(matching) > 1: raise ValueError('inventory_unavailable')
        if not matching: continue
        if matching[0].get('description') != provider.marker: raise ValueError('resource_ownership')
        if parts == ('networks', 'subnets') and serverless_addresses(provider):
            remaining.append('networks subnets:' + name)
            continue
        if remaining:
            remaining.append('networks:' + name)
            continue
        try:
            provider.run('compute', *parts, 'delete', name, *((delete_flag,) if delete_flag else ()))
        except InstallError:
            remaining.append(' '.join(parts) + ':' + name)
            continue
        after = provider.run(*args, parse=True)
        if (not isinstance(after, list) or any(not isinstance(row, dict) or not isinstance(row.get('name'), str)
                                               or not row['name'] for row in after)): raise ValueError('inventory_unavailable')
        if any(row.get('name') == name for row in after): remaining.append(' '.join(parts) + ':' + name)
    return remaining
