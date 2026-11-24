// Disposable-test backup cleanup only; the caller supplies an authenticated API transport.
export async function cleanupTestBackup({ projectId, installationId, protectedIds, verifyFresh, request, sleep = ms => new Promise(resolve => setTimeout(resolve, ms)) }) {
  if (!/^[a-z][a-z0-9-]{4,28}[a-z0-9]$/.test(projectId ?? '') || !/^[a-f0-9]{12}([a-f0-9]{12})?$/.test(installationId ?? '') || !Array.isArray(protectedIds) || protectedIds.some(id => !/^[a-f0-9]{12}([a-f0-9]{12})?$/.test(id)) || protectedIds.some(id => id.slice(0, 12) === installationId.slice(0, 12)) || typeof verifyFresh !== 'function' || typeof request !== 'function') throw new Error('invalid_scope');
  const short = installationId.slice(0, 12), bucket = `momso-backup-${short}`;
  const storage = `https://storage.googleapis.com/storage/v1/b/${bucket}`;
  const ring = `projects/${projectId}/locations/asia-northeast3/keyRings/momso-backup-${short}`;
  const kms = `https://cloudkms.googleapis.com/v1/${ring}`;
  if (await verifyFresh({ projectId, installationId }) !== true) throw new Error('ownership_unconfirmed');
  const metadata = await request('GET', storage);
  if (metadata?.name !== bucket || metadata?.labels?.['momso-install-id'] !== installationId || !/^[0-9]+$/.test(String(metadata.metageneration ?? ''))) throw new Error('ownership_unconfirmed');
  const keys = await request('GET', `${kms}/cryptoKeys`);
  if (!keys || keys.nextPageToken || (keys.cryptoKeys !== undefined && !Array.isArray(keys.cryptoKeys))) throw new Error('keys_unconfirmed');
  keys.cryptoKeys ??= [];
  if (keys.cryptoKeys.some(item => item.name !== `${ring}/cryptoKeys/backup` || item.labels?.['momso-install-id'] !== installationId || item.purpose !== 'ENCRYPT_DECRYPT')) throw new Error('keys_unconfirmed');
  const versions = keys.cryptoKeys.length ? await request('GET', `${kms}/cryptoKeys/backup/cryptoKeyVersions`) : { cryptoKeyVersions: [] };
  if (!versions || versions.nextPageToken || (versions.cryptoKeyVersions !== undefined && !Array.isArray(versions.cryptoKeyVersions))) throw new Error('keys_unconfirmed');
  versions.cryptoKeyVersions ??= [];
  for (const version of versions.cryptoKeyVersions) {
    if (!new RegExp(`^${ring}/cryptoKeys/backup/cryptoKeyVersions/[0-9]+$`).test(version.name ?? '') || !['ENABLED', 'DISABLED', 'DESTROY_SCHEDULED', 'DESTROYED'].includes(version.state)) throw new Error('keys_unconfirmed');
  }
  const objects = [], pages = new Set();
  let page = '';
  do {
    if (pages.has(page) || pages.size >= 1000) throw new Error('objects_unconfirmed');
    pages.add(page);
    const listing = await request('GET', `${storage}/o?versions=true${page ? `&pageToken=${encodeURIComponent(page)}` : ''}`);
    if (!listing || (listing.items !== undefined && !Array.isArray(listing.items))) throw new Error('objects_unconfirmed');
    for (const item of listing.items ?? []) {
      if (typeof item.name !== 'string' || !item.name || !/^[0-9]+$/.test(String(item.generation ?? ''))) throw new Error('objects_unconfirmed');
      objects.push({ name: item.name, generation: String(item.generation) });
    }
    page = listing.nextPageToken ?? '';
    if (typeof page !== 'string') throw new Error('objects_unconfirmed');
  } while (page);
  // A policy change does not affect already soft-deleted generations.
  const softObjects = [];
  page = '';
  const softPages = new Set();
  do {
    if (softPages.has(page) || softPages.size >= 1000) throw new Error('objects_unconfirmed');
    softPages.add(page);
    const listing = await request('GET', `${storage}/o?softDeleted=true${page ? `&pageToken=${encodeURIComponent(page)}` : ''}`);
    if (!listing || (listing.items !== undefined && !Array.isArray(listing.items))) throw new Error('objects_unconfirmed');
    softObjects.push(...(listing.items ?? []).map(item => ({ hardDeleteTime: item.hardDeleteTime ?? null })));
    page = listing.nextPageToken ?? '';
    if (typeof page !== 'string') throw new Error('objects_unconfirmed');
  } while (page);
  let bucketGeneration = String(metadata.metageneration);
  if (String(metadata.softDeletePolicy?.retentionDurationSeconds ?? '') !== '0') {
    const changed = await request('PATCH', `${storage}?ifMetagenerationMatch=${bucketGeneration}`, { softDeletePolicy: { retentionDurationSeconds: '0' } });
    if (changed?.name !== bucket || changed?.labels?.['momso-install-id'] !== installationId || String(changed?.softDeletePolicy?.retentionDurationSeconds) !== '0' || !/^[0-9]+$/.test(String(changed?.metageneration ?? ''))) throw new Error('policy_change_unconfirmed');
    bucketGeneration = String(changed.metageneration);
  }
  // A prior attempt may have changed the policy immediately before stopping.
  // Waiting again avoids racing its propagation on a fresh attempt.
  await sleep(30000);
  for (const version of versions.cryptoKeyVersions) {
    if (['ENABLED', 'DISABLED'].includes(version.state)) {
      const updated = await request('POST', `https://cloudkms.googleapis.com/v1/${version.name}:destroy`);
      if (updated?.name !== version.name || updated?.state !== 'DESTROY_SCHEDULED') throw new Error('key_destruction_unconfirmed');
    }
  }
  for (const object of objects) await request('DELETE', `${storage}/o/${encodeURIComponent(object.name)}?generation=${object.generation}&ifGenerationMatch=${object.generation}`);
  if (!softObjects.length) await request('DELETE', `${storage}?ifMetagenerationMatch=${bucketGeneration}`);
  return { bucket: softObjects.length ? 'retained_soft_deleted' : 'delete_requested', softDeleted: softObjects, keyVersions: versions.cryptoKeyVersions.map(version => ({ state: version.state === 'DESTROYED' ? 'DESTROYED' : 'DESTROY_SCHEDULED', billing: version.state === 'DESTROYED' ? 'not_active' : 'may_be_billable' })), keyRing: 'retained_pending_verification' };
}
