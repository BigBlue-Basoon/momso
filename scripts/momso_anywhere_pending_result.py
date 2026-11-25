"""Report deferred network cleanup only after the other cleanup attempts."""
from momso_anywhere_delete_result import deletion_result


def pending_result(provider, args, remaining, deletion):
    from momso_anywhere_network_cleanup import other_inventory, pending_serverless
    from momso_server_install import network_retry_command
    result = dict(deletion)
    result['resources'] = dict(deletion.get('resources') or {})
    result['billable'] = list(deletion.get('billable') or [])
    result['remaining'] = list(remaining)
    result.update(billingZero=None, retryable=True, reason='cleanup_pending')
    try:
        extra, versions = other_inventory(provider)
        for name, item in extra.items():
            old = result['resources'].get(name)
            # A backup-retention finding cannot be erased by a later, narrower
            # live-resource listing.
            if isinstance(old, dict) and old.get('status') in ('retained', 'unverified') and item.get('status') == 'deleted':
                continue
            if isinstance(old, dict) and old.get('status') == 'retained' and item.get('status') == 'retained':
                result['resources'][name] = {**item, **old}
                continue
            result['resources'][name] = item
        for version in versions:
            if version.get('billing') == 'billable':
                charge = {'resource': 'kms', 'endsAt': version.get('until')}
                if charge not in result['billable']: result['billable'].append(charge)
        others_absent = bool(extra) and all(item.get('status') == 'deleted' for item in extra.values()) and not versions
        if others_absent and all(item.get('status') == 'deleted' for name, item in result['resources'].items()
                                 if name not in ('network',) and isinstance(item, dict)):
            pending = pending_serverless(provider, remaining)
            if pending:
                result.update(pending)
                result.update(network_retry_command(args))
    except Exception:
        result['resources']['remaining_inventory'] = {'status': 'unverified', 'reason': 'inventory_unavailable'}
    bucket = result['resources'].get('bucket', {})
    if bucket.get('status') == 'retained' and not any(item.get('resource') == 'bucket' for item in result['billable']):
        result['billable'].append({'resource': 'bucket', 'endsAt': None})
    # Timed retention is still a real remaining resource even if the generic
    # normalizer considers its scheduled cleanup acceptable.
    for name, item in result['resources'].items():
        if isinstance(item, dict) and item.get('status') == 'retained' and name not in result['remaining']:
            result['remaining'].append(name)
    return deletion_result(result)
