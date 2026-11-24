// Shared customer-facing contract: unknown inputs never become free or absent.
const KMS_HOURLY_SOFTWARE_USD = 0.000082192;
const HOURS_PER_ESTIMATED_MONTH = 730;
// Seoul, Standard, on-demand: Google pricing page checked 2026-11-24.
const SEOUL_STANDARD_HOURLY_USD_PER_GIB = 0.000031507;
const ACTIVE_KEY_STATES = new Set(['ENABLED', 'DISABLED', 'DESTROY_SCHEDULED']);
const REQUIRED_RESOURCES = ['vm', 'disk', 'network', 'subnet', 'firewall', 'router', 'nat', 'gateway', 'accounts', 'roles', 'secrets', 'iam', 'bucket', 'kms'];

export function deletionPreview({ bucket = null, slots = null, versions = null, project = null } = {}) {
  const owned = bucket?.ownership === 'owned';
  const hasBackup = owned && slots?.complete === true && Array.isArray(slots.items)
    ? slots.items.length ? true : slots.softDeletedComplete === true ? !!slots.softDeletedCount : null
    : null;
  const dates = owned && slots?.complete === true && Array.isArray(slots.items)
    ? slots.items.filter(item => /^momso-backup\/slot-[0-6]\.enc$/.test(item.name ?? '') && typeof item.updated === 'string' && Number.isFinite(Date.parse(item.updated))).map(item => item.updated).sort((a, b) => Date.parse(a) - Date.parse(b))
    : [];
  const keyItems = versions?.complete === true && Array.isArray(versions.items) ? versions.items : null;
  const knownStates = keyItems && keyItems.every(item => ['ENABLED', 'DISABLED', 'DESTROY_SCHEDULED', 'DESTROYED'].includes(item.state));
  const knownPrices = knownStates && keyItems.every(item => item.state === 'DESTROYED' || item.protectionLevel === 'SOFTWARE');
  const active = knownStates ? keyItems.filter(item => ACTIVE_KEY_STATES.has(item.state)).length : null;
  const items = owned && typeof bucket?.location === 'string' && bucket.location.toUpperCase() === 'ASIA-NORTHEAST3' && slots?.complete === true && slots.softDeletedComplete === true && Array.isArray(slots.items) && Array.isArray(slots.softDeletedItems)
    ? [...slots.items, ...slots.softDeletedItems] : null;
  const storageKnown = items && items.every(item => item.storageClass === 'STANDARD' && /^\d+$/.test(String(item.size ?? '')) && Number.isSafeInteger(Number(item.size)));
  const bytes = storageKnown ? items.reduce((sum, item) => sum + Number(item.size), 0) : null;
  const storage = bytes !== null && Number.isSafeInteger(bytes) ? Number((bytes / 1073741824 * SEOUL_STANDARD_HOURLY_USD_PER_GIB * HOURS_PER_ESTIMATED_MONTH).toPrecision(6)) : null;
  const kms = knownPrices ? Number((active * KMS_HOURLY_SOFTWARE_USD * HOURS_PER_ESTIMATED_MONTH).toFixed(4)) : null;
  return {
    backupExists: hasBackup,
    lastBackupAt: dates.at(-1) ?? null,
    lastBackupValidity: dates.length ? 'not_verified' : 'unknown',
    vaultIdentity: 'must_retain_for_pause',
    estimatedMonthly: {
      currency: 'USD', kms, storage, subtotal: kms !== null && storage !== null ? Number((kms + storage).toPrecision(8)) : null,
      total: null, hoursPerMonth: HOURS_PER_ESTIMATED_MONTH,
      reason: 'other_charges_or_billable_generations_unverified', pricingCheckedAt: '2026-11-24',
      kmsPriceSource: 'https://cloud.google.com/kms/pricing', storagePriceSource: 'https://cloud.google.com/storage/pricing',
    },
    projectClosure: project?.createdByMomso === false ? 'not_suggested' : 'unverified',
  };
}

export function deletionResult({ mode, resources = {}, billable = [], inventoryComplete = false } = {}) {
  if (!['pause', 'delete_all'].includes(mode) || !Array.isArray(billable)) throw new Error('invalid_deletion_result');
  const safe = {};
  for (const [name, value] of Object.entries(resources)) {
    if (!/^[a-z][a-z0-9_]{0,60}$/.test(name)) continue;
    safe[name] = ['deleted', 'retained', 'unverified'].includes(value?.status)
      ? { status: value.status, reason: typeof value.reason === 'string' && /^[a-z0-9_]{1,80}$/.test(value.reason) ? value.reason : null }
      : { status: 'unverified', reason: null };
  }
  const charges = billable.map(item => ({ resource: typeof item?.resource === 'string' && /^[a-z0-9_]{1,60}$/.test(item.resource) ? item.resource : 'unknown',
    endsAt: typeof item?.endsAt === 'string' && /^\d{4}-\d\d-\d\dT\d\d:\d\d:\d\d(?:\.\d+)?Z$/.test(item.endsAt) ? item.endsAt : null }));
  const complete = inventoryComplete && REQUIRED_RESOURCES.every(key => safe[key] && safe[key].status !== 'unverified' &&
    (safe[key].status !== 'retained' || resources[key]?.billing === 'not_billable' || charges.some(item => item.resource === key))) &&
    charges.every(item => item.resource !== 'unknown');
  const allTimesKnown = charges.every(item => item.endsAt !== null);
  return { mode, resources: safe, billable: charges, billingZero: complete && charges.length === 0 ? true : complete && charges.length ? false : null,
    estimatedZeroAt: complete && charges.length && allTimesKnown ? charges.map(item => item.endsAt).sort().at(-1) : null };
}
