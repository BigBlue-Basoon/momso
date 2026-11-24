"""Normalize PC and mobile deletion results without inferring unobserved state."""


def deletion_result(value):
    result = dict(value)
    mode = 'delete_all' if result.get('mode') == 'discard' else result.get('mode')
    resources = dict(result.get('resources') or {})
    for item in result.get('retained') or []:
        if isinstance(item, dict) and item.get('resource') and item['resource'] not in resources:
            resources[item['resource']] = {'status': item.get('status'), 'reason': item.get('reason')}
    remaining = result.get('remaining') or []
    for name in remaining:
        resources[name] = {'status': 'retained', 'reason': 'cleanup_pending'}
    unverified = sorted(name for name, item in resources.items() if not isinstance(item, dict) or item.get('status') not in ('deleted', 'retained'))
    if not resources and not result.get('failed'):
        unverified.append('resources')
    from datetime import datetime
    import re
    def timed(name):
        charges = [item for item in result.get('billable') or [] if isinstance(item, dict) and item.get('resource') == name]
        if not charges:
            return False
        for item in charges:
            value = item.get('endsAt')
            if not isinstance(value, str) or not re.fullmatch(r'\d{4}-\d\d-\d\dT\d\d:\d\d:\d\d(?:\.\d+)?(?:Z|[+-]\d\d:\d\d)', value):
                return False
            try:
                datetime.fromisoformat(value.replace('Z', '+00:00'))
            except ValueError:
                return False
        return True
    retained = [name for name, item in resources.items() if isinstance(item, dict) and item.get('status') == 'retained' and not (mode == 'pause' and item.get('reason') == 'backup_recovery') and not timed(name)]
    outcome = 'failed' if result.get('failed') else 'remaining' if retained else 'deleted_unverified' if unverified else 'deleted'
    complete = outcome in ('deleted', 'deleted_unverified')
    reason = result.get('reason') or ((resources[retained[0]].get('reason') or 'cleanup_pending') if outcome == 'remaining' else None)
    retryable = False if complete else result.get('retryable') if isinstance(result.get('retryable'), bool) else True if outcome == 'remaining' or reason in ('vm_unreachable', 'network', 'google_unavailable') else None
    remaining = list(dict.fromkeys([*remaining, *retained]))
    result.update(mode=mode, status='complete' if complete else 'pending', outcome=outcome, unverified=unverified, complete=complete, retryable=retryable,
                  reason=reason, resources=resources, remaining=remaining, billable=result.get('billable') or [],
                  billingZero=result.get('billingZero'), estimatedZeroAt=result.get('estimatedZeroAt'))
    return result
