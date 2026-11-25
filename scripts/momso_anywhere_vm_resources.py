"""Names and ownership checks for resources provisioned for one VM."""
def names(provider):
    ident = provider.install_id[:12]
    machine = 'momso-' + ident
    return {'account': 'mv-' + ident, 'backup_account': 'mb-' + ident,
            'roles': ('momsoStop' + ident, 'momsoBackup' + ident, 'momsoUpdate' + ident),
            'vault_secret': 'momso-vault-' + ident,
            'gateway': 'momso-any-' + ident, 'gateway_account': 'ma-' + ident,
            'gateway_role': 'momsoAnywhere' + ident,
            'transport_secret': 'momso-transport-' + ident, 'session_secret': 'momso-session-' + ident,
            'wake': 'momso-wake-' + ident, 'wake_account': 'mw-' + ident, 'wake_role': 'momsoWake' + ident,
            'backup_bucket': 'momso-backup-' + ident, 'backup_ring': 'momso-backup-' + ident,
            'machine': machine, 'network': machine + '-net', 'subnet': machine + '-sub',
            'firewall': machine + '-iap', 'private_firewall': machine + '-private',
            'router': machine + '-router', 'nat': machine + '-nat'}


def destroy_identity(provider, *, discard_data):
    """Delete only exactly named and independently ownership-checked resources."""
    p = provider
    owned = names(p)
    emails = {owned[key] + '@' + p.project + '.iam.gserviceaccount.com' for key in ('account', 'backup_account')}
    accounts = p.run('iam', 'service-accounts', 'list', parse=True)
    roles = p.run('iam', 'roles', 'list', parse=True)
    secrets = p.run('secrets', 'list', parse=True) if discard_data else []
    if not all(isinstance(value, list) and all(isinstance(item, dict) for item in value)
               for value in (accounts, roles, secrets)):
        raise ValueError('vm_resource_inventory_unavailable')
    found_accounts = [x for x in accounts if x.get('email') in emails]
    found_roles = [x for x in roles if x.get('name', '').rsplit('/', 1)[-1] in owned['roles'] and not x.get('deleted')]
    secret = next((x for x in secrets if x.get('name', '').rsplit('/', 1)[-1] == owned['vault_secret']), None)
    for account in found_accounts:
        if p.run('iam', 'service-accounts', 'describe', account['email'], parse=True).get('description') != p.marker:
            raise ValueError('vm_account_ownership_mismatch')
    for role in found_roles:
        name = role['name'].rsplit('/', 1)[-1]
        if p.run('iam', 'roles', 'describe', name, parse=True).get('description') != p.marker:
            raise ValueError('vm_role_ownership_mismatch')
    if secret and secret.get('labels', {}).get('momso-install-id') != p.install_id:
        raise ValueError('vault_secret_ownership_mismatch')
    for account in found_accounts: p.run('iam', 'service-accounts', 'delete', account['email'])
    if discard_data:
        for role in found_roles: p.run('iam', 'roles', 'delete', role['name'].rsplit('/', 1)[-1])
    if secret: p.run('secrets', 'delete', owned['vault_secret'])
