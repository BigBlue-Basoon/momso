"""Fail-closed decision from independently verified backup observations."""
import datetime
import re


class RestorePlanError(ValueError):
    pass


def restore_plan(*, vault, bucket, key, slots):
    resources = (vault, bucket, key)
    if any(value not in ('absent', 'ready', 'unverified') for value in resources) or 'unverified' in resources:
        raise RestorePlanError('restore_assets_unverified')
    if all(value == 'absent' for value in resources):
        return {'restoreAvailable': False, 'lastBackupAt': None, 'requiresRestore': False}
    if any(value != 'ready' for value in resources):
        raise RestorePlanError('restore_assets_incomplete')
    if not isinstance(slots, list):
        raise RestorePlanError('restore_assets_unverified')
    times = []
    for item in slots:
        if not isinstance(item, dict) or not re.fullmatch(r'momso-backup/slot-[0-6]\.enc', str(item.get('name', ''))):
            continue
        stamp = item.get('updated')
        if not isinstance(stamp, str) or not re.fullmatch(r'\d{4}-\d\d-\d\dT\d\d:\d\d:\d\d(?:\.\d+)?(?:Z|[+-]\d\d:\d\d)', stamp):
            raise RestorePlanError('restore_assets_unverified')
        try:
            value = datetime.datetime.fromisoformat(stamp.replace('Z', '+00:00'))
        except ValueError:
            raise RestorePlanError('restore_assets_unverified') from None
        times.append((value, stamp))
    if not times:
        return {'restoreAvailable': False, 'lastBackupAt': None, 'requiresRestore': True}
    return {'restoreAvailable': True, 'lastBackupAt': max(times)[1], 'requiresRestore': True}
