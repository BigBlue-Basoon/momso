// Uninspected resources remain visible even when the deletion request completed.
export function deletionResult(input = {}) {
  const mode = input.mode === 'discard' ? 'delete_all' : input.mode;
  const resources = { ...(input.resources ?? {}) };
  for (const item of input.retained ?? []) {
    if (item?.resource && !resources[item.resource]) resources[item.resource] = { status: item.status, reason: item.reason ?? null };
  }
  for (const name of input.remaining ?? []) {
    if (resources[name]?.status !== 'retained') resources[name] = { status: 'retained', reason: 'cleanup_pending' };
  }
  const unverified = Object.entries(resources).filter(([, item]) => !item || !['deleted', 'retained'].includes(item.status)).map(([name]) => name).sort();
  if (!Object.keys(resources).length && !input.failed) unverified.push('resources');
  const billable = input.billable ?? [];
  const knownEnd = value => {
    if (typeof value !== 'string' || !/^\d{4}-\d\d-\d\dT\d\d:\d\d:\d\d(?:\.\d+)?(?:Z|[+-]\d\d:\d\d)$/.test(value) || !Number.isFinite(Date.parse(value))) return false;
    const [year, month, day] = value.slice(0, 10).split('-').map(Number);
    const calendar = new Date(0); calendar.setUTCFullYear(year, month - 1, day);
    return year >= 1 && calendar.getUTCFullYear() === year && calendar.getUTCMonth() === month - 1 && calendar.getUTCDate() === day;
  };
  const timed = name => { const charges = billable.filter(charge => charge?.resource === name); return charges.length > 0 && charges.every(charge => knownEnd(charge.endsAt)); };
  const retained = Object.entries(resources).filter(([name, item]) => item?.status === 'retained' && !(mode === 'pause' && item.reason === 'backup_recovery') && !timed(name)).map(([name]) => name);
  const outcome = input.failed ? 'failed' : retained.length ? 'remaining' : unverified.length ? 'deleted_unverified' : 'deleted';
  const complete = outcome === 'deleted' || outcome === 'deleted_unverified';
  const reason = input.reason ?? (outcome === 'remaining' ? resources[retained[0]]?.reason ?? 'cleanup_pending' : null);
  const retryable = complete ? false : typeof input.retryable === 'boolean' ? input.retryable : outcome === 'remaining' || ['vm_unreachable', 'network', 'google_unavailable'].includes(reason) ? true : null;
  return { ...input, mode, status: complete ? 'complete' : 'pending', outcome, unverified, complete, retryable, reason,
    resources, remaining: [...new Set([...(input.remaining ?? []), ...retained])], billable: input.billable ?? [], billingZero: input.billingZero ?? null,
    estimatedZeroAt: input.estimatedZeroAt ?? null };
}
