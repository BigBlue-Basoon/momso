"""Conservative IAM policy cleanup for PC deletion; no credential output."""
import json
import os
import tempfile
from pathlib import Path

from momso_anywhere_iam_policy import remove_and_verify
from momso_anywhere_vm_resources import names
from momso_server_install import REGION, ZONE


class PartialPolicyInventory(ValueError):
    def __init__(self, found):
        super().__init__('soft_deleted_bucket_policy_unavailable')
        self.found = found


class GcloudPolicies:
    def __init__(self, provider): self.provider = provider

    def read(self, resource):
        return self.provider.run(*resource[0], parse=True)

    def write(self, resource, policy):
        fd, path = tempfile.mkstemp(prefix='momso-iam-', suffix='.json')
        try:
            with os.fdopen(fd, 'w', encoding='utf-8') as stream:
                json.dump(policy, stream, separators=(',', ':'))
            self.provider.run(*resource[1], path, *resource[2])
        finally:
            Path(path).unlink(missing_ok=True)


def soft_deleted_buckets(p):
    """Read the project's soft-deleted bucket metadata without restoring it."""
    raw = p.run('storage', 'ls', '--buckets', '--soft-deleted', '--json')
    text = raw.decode('utf-8') if isinstance(raw, bytes) else raw
    if not isinstance(text, str): raise ValueError('invalid_bucket_inventory')
    records, decoder = [], json.JSONDecoder()
    while text.strip():
        text = text.lstrip()
        value, end = decoder.raw_decode(text)
        records.extend(value if isinstance(value, list) else [value])
        text = text[end:]
    expected = names(p)['backup_bucket']
    found = []
    for row in records:
        if not isinstance(row, dict): raise ValueError('invalid_bucket_inventory')
        metadata = row.get('metadata', row)
        if not isinstance(metadata, dict): raise ValueError('invalid_bucket_inventory')
        name = metadata.get('name') or row.get('name')
        if not isinstance(name, str): raise ValueError('invalid_bucket_inventory')
        if name == expected: found.append({'name': name, 'hardDeleteTime': metadata.get('hardDeleteTime')})
    return found


def owned_policy_resources(p):
    """Enumerate the known installation resources; reject ownership collisions."""
    n = names(p)
    resources = [(('projects', 'get-iam-policy', p.project), ('projects', 'set-iam-policy', p.project), ())]
    instances = p.run('compute', 'instances', 'list', f'--zones={ZONE}', parse=True)
    services = p.run('run', 'services', 'list', f'--region={REGION}', parse=True)
    buckets = p.run('storage', 'buckets', 'list', parse=True)
    secrets = p.run('secrets', 'list', parse=True)
    rings = p.run('kms', 'keyrings', 'list', f'--location={REGION}', parse=True)
    if any(not isinstance(items, list) or any(not isinstance(item, dict) for item in items) for items in (instances, services, buckets, secrets, rings)):
        raise ValueError('iam_bindings_unverified')
    instance = next((x for x in instances if x.get('name') == n['machine']), None)
    if instance:
        if instance.get('labels', {}).get('momso-install-id') != p.install_id: raise ValueError('resource_ownership')
        resources.append((('compute', 'instances', 'get-iam-policy', n['machine'], f'--zone={ZONE}'), ('compute', 'instances', 'set-iam-policy', n['machine']), (f'--zone={ZONE}',)))
    for service_name in (n['gateway'], n['wake']):
        service = next((x for x in services if x.get('metadata', {}).get('name') == service_name), None)
        if service:
            if service.get('metadata', {}).get('labels', {}).get('momso-install-id') != p.install_id: raise ValueError('resource_ownership')
            resources.append((('run', 'services', 'get-iam-policy', service_name, f'--region={REGION}'), ('run', 'services', 'set-iam-policy', service_name), (f'--region={REGION}',)))
    bucket = next((x for x in buckets if x.get('name') == n['backup_bucket']), None)
    if bucket:
        bucket = p.run('storage', 'buckets', 'describe', 'gs://' + n['backup_bucket'], parse=True)
        if bucket.get('labels', {}).get('momso-install-id') != p.install_id: raise ValueError('resource_ownership')
        resources.append((('storage', 'buckets', 'get-iam-policy', 'gs://' + n['backup_bucket']), ('storage', 'buckets', 'set-iam-policy', 'gs://' + n['backup_bucket']), ()))
    for secret_name in (n['vault_secret'], n['transport_secret'], n['session_secret']):
        secret = next((x for x in secrets if x.get('name', '').endswith('/secrets/' + secret_name)), None)
        if secret:
            if secret.get('labels', {}).get('momso-install-id') != p.install_id: raise ValueError('resource_ownership')
            resources.append((('secrets', 'get-iam-policy', secret_name), ('secrets', 'set-iam-policy', secret_name), ()))
    ring = next((x for x in rings if x.get('name', '').endswith('/keyRings/' + n['backup_ring'])), None)
    if ring:
        keys = p.run('kms', 'keys', 'list', f'--location={REGION}', f"--keyring={n['backup_ring']}", parse=True)
        if not isinstance(keys, list) or any(not isinstance(item, dict) for item in keys): raise ValueError('iam_bindings_unverified')
        key = next((x for x in keys if x.get('name', '').endswith('/cryptoKeys/backup')), None)
        if key:
            actual = p.run('kms', 'keys', 'describe', 'backup', f'--location={REGION}', f"--keyring={n['backup_ring']}", parse=True)
            if actual.get('labels', {}).get('momso-install-id') != p.install_id: raise ValueError('resource_ownership')
            flags = (f'--location={REGION}', f"--keyring={n['backup_ring']}")
            resources.append((('kms', 'keys', 'get-iam-policy', 'backup', *flags), ('kms', 'keys', 'set-iam-policy', 'backup'), flags))
            resources.append((('kms', 'keyrings', 'get-iam-policy', n['backup_ring'], f'--location={REGION}'), ('kms', 'keyrings', 'set-iam-policy', n['backup_ring']), (f'--location={REGION}',)))
        else:
            resources.append((('kms', 'keyrings', 'get-iam-policy', n['backup_ring'], f'--location={REGION}'), ('kms', 'keyrings', 'set-iam-policy', n['backup_ring']), (f'--location={REGION}',)))
    return resources


def inspect_installation_bindings(p):
    """Read all surviving installation policies; never modify any binding."""
    from momso_anywhere_iam_policy import without_accounts
    n = names(p)
    emails = [n[key] + '@' + p.project + '.iam.gserviceaccount.com'
              for key in ('account', 'backup_account', 'gateway_account', 'wake_account')]
    roles = {f'projects/{p.project}/roles/{role}' for role in
             (n['gateway_role'], n['wake_role'], *n['roles'])}
    found = []
    transport = GcloudPolicies(p)
    for resource in owned_policy_resources(p):
        policy = transport.read(resource)
        filtered = without_accounts(policy, emails)
        bindings = policy.get('bindings', [])
        if filtered != policy or any(isinstance(binding.get('role'), str) and
                                     any(binding['role'] == role or binding['role'].startswith(role + '_withcond_')
                                         for role in roles) for binding in bindings):
            found.append('/'.join(resource[0][:3]))
    # Do not restore a soft-deleted bucket merely to inspect its policy.
    if soft_deleted_buckets(p):
        raise PartialPolicyInventory(found)
    return found


def remove_installation_bindings(p):
    n = names(p)
    accounts = p.run('iam', 'service-accounts', 'list', parse=True)
    if not isinstance(accounts, list) or any(not isinstance(item, dict) for item in accounts): raise ValueError('iam_bindings_unverified')
    emails = [n[key] + '@' + p.project + '.iam.gserviceaccount.com' for key in ('account', 'backup_account', 'gateway_account', 'wake_account')]
    account_resources = []
    for account in accounts:
        if account.get('email') in emails:
            actual = p.run('iam', 'service-accounts', 'describe', account['email'], parse=True)
            if actual.get('description') != p.marker: raise ValueError('resource_ownership')
            email = account['email']
            account_resources.append((('iam', 'service-accounts', 'get-iam-policy', email), ('iam', 'service-accounts', 'set-iam-policy', email), ()))
    remove_and_verify(GcloudPolicies(p), [*owned_policy_resources(p), *account_resources], emails)
