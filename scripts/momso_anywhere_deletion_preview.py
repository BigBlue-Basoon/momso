"""Read-only backup preview for the PC installer; no object content is read."""
import datetime
import re

from momso_server_install import REGION, InstallError


class OwnershipError(Exception):
    pass


def preview(provider):
    ident = getattr(provider, 'install_id', '')
    if not re.fullmatch(r'[a-f0-9]{12}', ident):
        raise InstallError('설치 번호를 확인하지 못했습니다.')
    name = 'momso-backup-' + ident
    output = {
        'backupExists': None, 'lastBackupAt': None, 'lastBackupValidity': 'unknown',
        'vaultIdentity': 'must_retain_for_pause',
        'estimatedMonthly': {'currency': 'USD', 'kms': None, 'storage': None, 'subtotal': None, 'total': None,
                             'hoursPerMonth': 730, 'reason': 'other_charges_or_billable_generations_unverified', 'pricingCheckedAt': '2026-11-24',
                             'kmsPriceSource': 'https://cloud.google.com/kms/pricing',
                             'storagePriceSource': 'https://cloud.google.com/storage/pricing'},
        'projectClosure': 'unverified',
    }
    try:
        buckets = provider.run('storage', 'buckets', 'list', parse=True)
        if not isinstance(buckets, list):
            raise ValueError('unavailable')
        if any(item.get('name') == name for item in buckets):
            bucket = provider.run('storage', 'buckets', 'describe', 'gs://' + name, parse=True)
            if bucket.get('name') != name or bucket.get('labels', {}).get('momso-install-id') != ident:
                raise OwnershipError('백업의 소유권을 확인하지 못했습니다.')
            live = provider.run('storage', 'objects', 'list', 'gs://' + name + '/**', parse=True)
            if not isinstance(live, list):
                raise ValueError('unavailable')
            times = []
            for item in live:
                name_value, stamp = item.get('name', ''), item.get('updated') or item.get('updateTime')
                if re.search(r'(?:^|/)momso-backup/slot-[0-6]\.enc$', name_value) and isinstance(stamp, str):
                    try:
                        times.append((datetime.datetime.fromisoformat(stamp.replace('Z', '+00:00')), stamp))
                    except ValueError:
                        pass
            if times:
                output['lastBackupAt'] = max(times, key=lambda value: value[0])[1]
                output['lastBackupValidity'] = 'not_verified'
            if live:
                output['backupExists'] = True
            else:
                try:
                    soft = provider.run('storage', 'objects', 'list', 'gs://' + name + '/**', '--soft-deleted', parse=True)
                    # The ordinary CLI listing does not establish that every
                    # noncurrent generation was enumerated; empty stays unknown.
                    if isinstance(soft, list) and soft:
                        output['backupExists'] = True
                except Exception:
                    pass
    except OwnershipError as error:
        raise InstallError(str(error)) from None
    except Exception:
        pass
    try:
        keys = provider.run('kms', 'keys', 'list', f'--location={REGION}', f'--keyring={name}', parse=True)
        if not isinstance(keys, list):
            raise ValueError('unavailable')
        expected = f'projects/{provider.project}/locations/{REGION}/keyRings/{name}/cryptoKeys/backup'
        matching = [key for key in keys if key.get('name') == expected]
        if matching:
            if matching[0].get('labels', {}).get('momso-install-id') != ident:
                raise OwnershipError('암호화 키의 소유권을 확인하지 못했습니다.')
            versions = provider.run('kms', 'keys', 'versions', 'list', '--key=backup', f'--keyring={name}', f'--location={REGION}', parse=True)
            known = {'ENABLED', 'DISABLED', 'DESTROY_SCHEDULED', 'DESTROYED'}
            if isinstance(versions, list) and all(v.get('state') in known and (v.get('state') == 'DESTROYED' or v.get('protectionLevel') == 'SOFTWARE') for v in versions):
                active = sum(v['state'] != 'DESTROYED' for v in versions)
                output['estimatedMonthly']['kms'] = round(active * 0.000082192 * 730, 4)
    except OwnershipError as error:
        raise InstallError(str(error)) from None
    except Exception:
        pass
    from momso_anywhere_project_exclusivity import inspect_project
    output['projectExclusivity'] = inspect_project(provider)
    if output['projectExclusivity']['status'] == 'not_exclusive':
        output['projectClosure'] = 'not_suggested'
    return output
