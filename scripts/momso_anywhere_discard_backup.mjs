import { cleanupTestBackup } from './momso_anywhere_test_backup_cleanup_s117.mjs';

export async function discardBackup({ projectId, installationId, request, sleep, previous = null }) {
  sleep ??= ms => new Promise(resolve => setTimeout(resolve, ms));
  const short = installationId.slice(0, 12);
  const storage = `https://storage.googleapis.com/storage/v1/b/momso-backup-${short}`;
  const ring = `projects/${projectId}/locations/asia-northeast3/keyRings/momso-backup-${short}`;
  const key = `https://cloudkms.googleapis.com/v1/${ring}/cryptoKeys/backup`;
  const result = { mode: 'delete_all', resources: { bucket: { status: 'unverified', reason: 'not_verified' }, kms: { status: 'unverified', reason: 'not_verified' } }, billable: [], billingZero: null, estimatedZeroAt: null };
  const unverified = () => ({ ...result, billable: Array.isArray(previous?.billable) ? previous.billable.filter(item => ['bucket', 'kms'].includes(item?.resource) && (item.endsAt === null || typeof item.endsAt === 'string')).slice(0, 100) : [] });
  let bucket;
  try { bucket = await request('GET', storage); } catch { return unverified(); }
  if (bucket && (bucket.name !== `momso-backup-${short}` || bucket.labels?.['momso-install-id'] !== installationId)) {
    result.resources.bucket.reason = 'ownership_unconfirmed'; return unverified();
  }
  let keyInfo, versions;
  try {
    keyInfo = await request('GET', key);
    if (keyInfo && (keyInfo.name !== `${ring}/cryptoKeys/backup` || keyInfo.labels?.['momso-install-id'] !== installationId || keyInfo.purpose !== 'ENCRYPT_DECRYPT')) {
      result.resources.kms.reason = 'ownership_unconfirmed'; return unverified();
    }
    versions = keyInfo ? await request('GET', `${key}/cryptoKeyVersions`) : { cryptoKeyVersions: [] };
    if (!versions || (versions.cryptoKeyVersions !== undefined && !Array.isArray(versions.cryptoKeyVersions)) || versions.nextPageToken) throw new Error('invalid');
    versions.cryptoKeyVersions ??= [];
    if (versions.cryptoKeyVersions.some(v => !new RegExp(`^${ring}/cryptoKeys/backup/cryptoKeyVersions/[0-9]+$`).test(v.name ?? '') || !['ENABLED', 'DISABLED', 'DESTROY_SCHEDULED', 'DESTROYED'].includes(v.state))) throw new Error('invalid');
  } catch { return unverified(); }
  let cleanup = null;
  if (bucket) {
    try {
      const cleaned = await cleanupTestBackup({ projectId, installationId, protectedIds: [], verifyFresh: async () => true,
        request: async (method, url, body) => (await request(method, url, body)) ?? (url.endsWith('/cryptoKeys') ? { cryptoKeys: [] } : null), sleep });
      cleanup = cleaned;
      result.resources.bucket = { status: cleaned.softDeleted.length ? 'retained' : 'unverified', reason: cleaned.softDeleted.length ? 'soft_delete_retention' : 'delete_not_verified' };
      for (const item of cleaned.softDeleted) result.billable.push({ resource: 'bucket', endsAt: item.hardDeleteTime });
    } catch { result.resources.bucket.reason = 'cleanup_unverified'; }
  } else {
    // A missing bucket may still have soft-deleted generations: never claim zero.
    result.resources.bucket.reason = 'missing_or_soft_deleted';
  }
  if (!bucket || cleanup) {
    try {
      const list = async soft => {
        let page = '', all = [], seen = new Set();
        do {
          if (seen.has(page) || seen.size >= 1000) throw new Error('invalid');
          seen.add(page);
          const q = new URLSearchParams({ project: projectId, prefix: `momso-backup-${short}` });
          if (soft) q.set('softDeleted', 'true');
          if (page) q.set('pageToken', page);
          const value = await request('GET', `https://storage.googleapis.com/storage/v1/b?${q}`);
          if (!value || (value.items !== undefined && !Array.isArray(value.items))) throw new Error('invalid');
          all.push(...(value.items ?? []).filter(x => x.name === `momso-backup-${short}`));
          page = value.nextPageToken ?? '';
          if (typeof page !== 'string') throw new Error('invalid');
        } while (page);
        return all;
      };
      const [live, soft] = await Promise.all([list(false), list(true)]);
      if ([...live, ...soft].some(x => x.labels?.['momso-install-id'] !== installationId)) throw new Error('invalid');
      if (live.length && !cleanup?.softDeleted.length) result.resources.bucket = { status: 'retained', reason: 'bucket_still_exists' };
      else if (soft.length) {
        result.resources.bucket = { status: 'retained', reason: 'soft_delete_retention' };
        for (const item of soft) result.billable.push({ resource: 'bucket', endsAt: typeof item.hardDeleteTime === 'string' ? item.hardDeleteTime : null });
      } else if (!cleanup?.softDeleted.length) result.resources.bucket = { status: 'deleted', reason: null };
    } catch { /* Listing failure cannot prove absence or zero cost. */ }
  }
  // Previously observed soft deletions may outlive their bucket. Do not lose
  // their retention deadline merely because a later bucket GET returns 404.
  if (Array.isArray(previous?.billable)) for (const item of previous.billable) {
    if (item?.resource !== 'bucket') continue;
    const at = typeof item.endsAt === 'string' && Number.isFinite(Date.parse(item.endsAt)) ? item.endsAt : null;
    if (at === null || Date.parse(at) > Date.now()) {
      result.resources.bucket = { status: 'retained', reason: 'soft_delete_retention' };
      if (!result.billable.some(x => x.resource === 'bucket' && x.endsAt === at)) result.billable.push({ resource: 'bucket', endsAt: at });
    }
  }
  if (keyInfo) {
    try {
      // Retry key scheduling independently if a bucket was already deleted.
      const current = await request('GET', `${key}/cryptoKeyVersions`);
      if (!current || (current.cryptoKeyVersions !== undefined && !Array.isArray(current.cryptoKeyVersions)) || current.nextPageToken) throw new Error('invalid');
      current.cryptoKeyVersions ??= [];
      if (current.cryptoKeyVersions.some(v => !new RegExp(`^${ring}/cryptoKeys/backup/cryptoKeyVersions/[0-9]+$`).test(v.name ?? '') || !['ENABLED', 'DISABLED', 'DESTROY_SCHEDULED', 'DESTROYED'].includes(v.state))) throw new Error('invalid');
      for (const version of current.cryptoKeyVersions) if (['ENABLED', 'DISABLED'].includes(version.state)) {
        const value = await request('POST', `https://cloudkms.googleapis.com/v1/${version.name}:destroy`);
        if (value?.name !== version.name || value.state !== 'DESTROY_SCHEDULED') throw new Error('invalid');
      }
      const after = await request('GET', `${key}/cryptoKeyVersions`);
      if (!after || (after.cryptoKeyVersions !== undefined && !Array.isArray(after.cryptoKeyVersions)) || after.nextPageToken) throw new Error('invalid');
      after.cryptoKeyVersions ??= [];
      for (const version of after.cryptoKeyVersions) {
        if (!new RegExp(`^${ring}/cryptoKeys/backup/cryptoKeyVersions/[0-9]+$`).test(version.name ?? '') || !['DESTROY_SCHEDULED', 'DESTROYED'].includes(version.state)) throw new Error('invalid');
        if (version.state === 'DESTROY_SCHEDULED') result.billable.push({ resource: 'kms', endsAt: typeof version.destroyTime === 'string' ? version.destroyTime : null });
      }
      result.resources.kms = { status: 'retained', reason: after.cryptoKeyVersions.some(v => v.state === 'DESTROY_SCHEDULED') ? 'destruction_scheduled' : 'keyring_not_deleted' };
      if (after.cryptoKeyVersions.every(v => v.state === 'DESTROYED')) {
        const finish = async operation => {
          if (!operation?.name || !new RegExp(`^projects/${projectId}/locations/asia-northeast3/operations/[A-Za-z0-9._-]+$`).test(operation.name)) throw new Error('invalid');
          for (let i = 0; i < 30; i++) {
            const value = await request('GET', `https://cloudkms.googleapis.com/v1/${operation.name}`);
            if (value?.done === true) { if (value.error) throw new Error('failed'); return; }
            await sleep(1000);
          }
          throw new Error('pending');
        };
        for (const version of after.cryptoKeyVersions) await finish(await request('DELETE', `https://cloudkms.googleapis.com/v1/${version.name}`));
        const empty = await request('GET', `${key}/cryptoKeyVersions`);
        if (!empty || empty.nextPageToken || (empty.cryptoKeyVersions !== undefined && !Array.isArray(empty.cryptoKeyVersions)) || (empty.cryptoKeyVersions ?? []).length) throw new Error('invalid');
        await finish(await request('DELETE', key));
        const keys = await request('GET', `https://cloudkms.googleapis.com/v1/${ring}/cryptoKeys`);
        if (!keys || keys.nextPageToken || (keys.cryptoKeys !== undefined && !Array.isArray(keys.cryptoKeys)) || (keys.cryptoKeys ?? []).length) throw new Error('invalid');
        await finish(await request('DELETE', `https://cloudkms.googleapis.com/v1/${ring}`));
        result.resources.kms = { status: 'deleted', reason: null };
      }
    } catch { result.resources.kms.reason = 'cleanup_unverified'; }
  } else {
    result.resources.kms.reason = 'key_missing_or_unavailable';
    try {
      let page = '', rings = [], seen = new Set();
      do {
        if (seen.has(page) || seen.size >= 1000) throw new Error('invalid');
        seen.add(page);
        const query = page ? `?pageToken=${encodeURIComponent(page)}` : '';
        const value = await request('GET', `https://cloudkms.googleapis.com/v1/projects/${projectId}/locations/asia-northeast3/keyRings${query}`);
        if (!value || (value.keyRings !== undefined && !Array.isArray(value.keyRings))) throw new Error('invalid');
        rings.push(...(value.keyRings ?? []));
        page = value.nextPageToken ?? '';
        if (typeof page !== 'string') throw new Error('invalid');
      } while (page);
      if (!rings.some(item => item.name === ring)) result.resources.kms = { status: 'deleted', reason: null };
    } catch { /* A missing or inaccessible key ring is not evidence of removal. */ }
  }
  result.resources.secret_vault = { status: 'unverified', reason: 'not_inspected' };
  const backupHandled = ['bucket', 'kms'].every(name => result.resources[name].status === 'deleted' || ['soft_delete_retention', 'destruction_scheduled', 'keyring_not_deleted'].includes(result.resources[name].reason));
  if (backupHandled) {
    try {
      const name = `projects/${projectId}/secrets/momso-vault-${short}`;
      const url = `https://secretmanager.googleapis.com/v1/${name}`;
      const secret = await request('GET', url);
      if (secret) {
        if (!new RegExp(`^projects/(?:${projectId}|[0-9]+)/secrets/momso-vault-${short}$`).test(secret.name ?? '') || secret.labels?.['momso-install-id'] !== installationId || typeof secret.etag !== 'string' || !secret.etag) throw new Error('ownership');
        await request('DELETE', `${url}?etag=${encodeURIComponent(secret.etag)}`);
        if (await request('GET', url)) throw new Error('pending');
        result.resources.secret_vault = { status: 'deleted', reason: null };
      } else {
        let page = '', names = [], seen = new Set();
        do {
          if (seen.has(page) || seen.size >= 1000) throw new Error('invalid');
          seen.add(page);
          const response = await request('GET', `https://secretmanager.googleapis.com/v1/projects/${projectId}/secrets${page ? `?pageToken=${encodeURIComponent(page)}` : ''}`);
          if (!response || (response.secrets !== undefined && !Array.isArray(response.secrets))) throw new Error('invalid');
          names.push(...(response.secrets ?? []).map(item => item.name));
          page = response.nextPageToken ?? '';
          if (typeof page !== 'string') throw new Error('invalid');
        } while (page);
        if (!names.some(item => typeof item === 'string' && item.endsWith(`/secrets/momso-vault-${short}`))) result.resources.secret_vault = { status: 'deleted', reason: null };
      }
    } catch { result.resources.secret_vault.reason = 'cleanup_unverified'; }
  }
  if (result.resources.kms.reason === 'cleanup_unverified' && Array.isArray(previous?.billable)) {
    for (const item of previous.billable) {
      if (item?.resource !== 'kms') continue;
      const at = typeof item.endsAt === 'string' && Number.isFinite(Date.parse(item.endsAt)) ? item.endsAt : null;
      if (at === null || Date.parse(at) > Date.now()) result.billable.push({ resource: 'kms', endsAt: at });
    }
  }
  const bucketCharges = result.billable.filter(x => x.resource === 'bucket');
  if (bucketCharges.length) {
    const allKnown = bucketCharges.every(x => typeof x.endsAt === 'string' && Number.isFinite(Date.parse(x.endsAt)));
    result.billable = result.billable.filter(x => x.resource !== 'bucket');
    result.billable.push({ resource: 'bucket', endsAt: allKnown ? bucketCharges.map(x => x.endsAt).sort((a, b) => Date.parse(a) - Date.parse(b)).at(-1) : null });
  }
  const backupKnown = ['bucket', 'kms'].every(name => result.resources[name].status === 'deleted' || ['soft_delete_retention', 'destruction_scheduled', 'keyring_not_deleted'].includes(result.resources[name].reason));
  const datesKnown = result.billable.every(x => typeof x.endsAt === 'string' && Number.isFinite(Date.parse(x.endsAt)));
  result.backupBillingZero = result.billable.length ? false : backupKnown && ['bucket', 'kms'].every(name => result.resources[name].status === 'deleted') ? true : null;
  result.backupChargesEndAt = backupKnown && result.billable.length && datesKnown
    ? result.billable.map(x => x.endsAt).sort((a, b) => Date.parse(a) - Date.parse(b)).at(-1) : null;
  return result;
}
