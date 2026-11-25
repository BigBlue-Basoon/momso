"""Inspect preserved PC-install assets without changing them."""
import re
from momso_anywhere_restore_plan import RestorePlanError, restore_plan
from momso_server_install import REGION


DESTROYED_KEY_STATES = frozenset(('DESTROY_SCHEDULED', 'DESTROYED'))


def discover_preserved(*, buckets, secrets, keys, instances, destroyed_keys=frozenset()):
    """destroyed_keys: installations whose backup key has no usable version.

    After "delete everything" the bucket is soft-deleted (absent from the
    bucket list) and the vault secret is gone, but Cloud KMS keeps the key
    forever with its versions scheduled for destruction. Such a remnant is
    not a paused installation and must not block a new install (S180).
    """
    if not all(isinstance(items, list) and all(isinstance(item, dict) for item in items) for items in (buckets, secrets, keys, instances)):
        raise RestorePlanError('restore_assets_unverified')
    candidates = {}
    def collect(items, kind):
        for item in items:
            labels = item.get('labels')
            if not isinstance(labels, dict): continue
            ident = labels.get('momso-install-id', '')
            if not isinstance(ident, str) or not re.fullmatch(r'[a-f0-9]{12}(?:[a-f0-9]{12})?', ident): continue
            short = ident[:12]
            name = item.get('name', '')
            expected = {'bucket': 'momso-backup-' + short, 'vault': 'momso-vault-' + short, 'key': 'backup'}[kind]
            if kind == 'key':
                if not name.endswith('/keyRings/momso-backup-' + short + '/cryptoKeys/backup'): continue
            elif name.rsplit('/', 1)[-1] != expected:
                continue
            candidates.setdefault(ident, set()).add(kind)
    collect(buckets, 'bucket'); collect(secrets, 'vault'); collect(keys, 'key')
    result = []
    for ident, found in sorted(candidates.items()):
        name = 'momso-' + ident[:12]
        if any(item.get('labels', {}).get('momso-install-id') == ident for item in instances if isinstance(item.get('labels'), dict)):
            continue
        matches = [item for item in instances if item.get('name', '').rsplit('/', 1)[-1] == name]
        if matches:
            if any(not isinstance(item.get('labels'), dict) or item['labels'].get('momso-install-id') != ident for item in matches):
                raise RestorePlanError('resource_ownership')
            continue
        if found == {'key'} and ident in destroyed_keys:
            continue
        result.append({'installationId': ident, 'assets': 'complete' if len(found) == 3 else 'incomplete',
                       'pcIdCompatible': True})
    return result


def discover_in_project(provider):
    """List candidates without changing customer resources or guessing ownership."""
    try:
        buckets = provider.run('storage', 'buckets', 'list', parse=True)
        secrets = provider.run('secrets', 'list', parse=True)
        rings = provider.run('kms', 'keyrings', 'list', f'--location={REGION}', parse=True)
        instances = provider.run('compute', 'instances', 'list', parse=True)
        if not all(isinstance(value, list) for value in (buckets, secrets, rings, instances)):
            raise RestorePlanError('restore_assets_unverified')
        keys = []
        destroyed = set()
        for ring in rings:
            name = ring.get('name', '').rsplit('/', 1)[-1]
            if not re.fullmatch(r'momso-backup-[a-f0-9]{12}', name):
                continue
            found = provider.run('kms', 'keys', 'list', f'--location={REGION}', f'--keyring={name}', parse=True)
            if not isinstance(found, list):
                raise RestorePlanError('restore_assets_unverified')
            keys.extend(found)
            for key in found:
                ident = key.get('labels', {}).get('momso-install-id', '') if isinstance(key, dict) and isinstance(key.get('labels'), dict) else ''
                if not (isinstance(key, dict) and key.get('name', '').endswith('/keyRings/' + name + '/cryptoKeys/backup')
                        and isinstance(ident, str) and ident[:12] == name.removeprefix('momso-backup-')):
                    continue
                versions = provider.run('kms', 'keys', 'versions', 'list', '--key=backup', f'--keyring={name}', f'--location={REGION}', parse=True)
                if not isinstance(versions, list) or any(not isinstance(item, dict) for item in versions):
                    raise RestorePlanError('restore_assets_unverified')
                if all(item.get('state') in DESTROYED_KEY_STATES for item in versions):
                    destroyed.add(ident)
        return discover_preserved(buckets=buckets, secrets=secrets, keys=keys, instances=instances, destroyed_keys=destroyed)
    except RestorePlanError:
        raise
    except Exception:
        raise RestorePlanError('restore_assets_unverified') from None


def inspect_restore(provider):
    ident = provider.install_id
    if not re.fullmatch(r'[a-f0-9]{12}(?:[a-f0-9]{12})?', ident):
        raise RestorePlanError('restore_assets_unverified')
    name = 'momso-backup-' + ident[:12]
    vault_name = 'momso-vault-' + ident[:12]
    try:
        buckets = provider.run('storage', 'buckets', 'list', parse=True)
        secrets = provider.run('secrets', 'list', parse=True)
        rings = provider.run('kms', 'keyrings', 'list', f'--location={REGION}', parse=True)
        if not all(isinstance(value, list) for value in (buckets, secrets, rings)):
            raise RestorePlanError('restore_assets_unverified')
        bucket = next((item for item in buckets if item.get('name') == name), None)
        vault = next((item for item in secrets if item.get('name', '').endswith('/secrets/' + vault_name)), None)
        ring = next((item for item in rings if item.get('name', '').endswith('/keyRings/' + name)), None)
        key = None
        if ring:
            keys = provider.run('kms', 'keys', 'list', f'--location={REGION}', f'--keyring={name}', parse=True)
            if not isinstance(keys, list): raise RestorePlanError('restore_assets_unverified')
            key = next((item for item in keys if item.get('name', '').endswith('/keyRings/' + name + '/cryptoKeys/backup')), None)
        for item in (bucket, vault, key):
            if item and item.get('labels', {}).get('momso-install-id') != ident:
                raise RestorePlanError('resource_ownership')
        if not all((bucket, vault, key)):
            raise RestorePlanError('restore_assets_incomplete')
        versions = provider.run('secrets', 'versions', 'list', vault_name, parse=True)
        key_versions = provider.run('kms', 'keys', 'versions', 'list', '--key=backup', f'--keyring={name}', f'--location={REGION}', parse=True)
        if not isinstance(versions, list) or not isinstance(key_versions, list):
            raise RestorePlanError('restore_assets_unverified')
        if not any(item.get('state') == 'ENABLED' and item.get('name', '').endswith('/versions/1') for item in versions) or not any(item.get('state') == 'ENABLED' for item in key_versions):
            raise RestorePlanError('restore_assets_incomplete')
        slots = provider.run('storage', 'objects', 'list', 'gs://' + name + '/momso-backup/**', parse=True)
        if not isinstance(slots, list): raise RestorePlanError('restore_assets_unverified')
        prefix = 'gs://' + name + '/'
        slots = [{**item, 'name': item.get('name', '').removeprefix(prefix), 'updated': item.get('updated') or item.get('updateTime')} for item in slots if isinstance(item, dict)]
        return restore_plan(vault='ready', bucket='ready', key='ready', slots=slots)
    except RestorePlanError:
        raise
    except Exception:
        raise RestorePlanError('restore_assets_unverified') from None
