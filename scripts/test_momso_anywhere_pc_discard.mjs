import test from 'node:test';
import assert from 'node:assert/strict';
import { discardBackup } from './momso_anywhere_pc_discard.mjs';

const id = 'a'.repeat(12), projectId = 'fixture-project';
const ring = `projects/${projectId}/locations/asia-northeast3/keyRings/momso-backup-${id}`;
const key = `${ring}/cryptoKeys/backup`, version = `${key}/cryptoKeyVersions/1`;

function fixture({ foreign = false, missingBucket = false } = {}) {
  const calls = []; let state = 'ENABLED';
  const request = async (method, url) => {
    calls.push({ method, url });
    if (url.includes('storage.googleapis.com') && !url.includes('/o')) {
      if (method === 'DELETE') return {};
      return missingBucket ? null : { name: `momso-backup-${id}`, labels: { 'momso-install-id': foreign ? 'b'.repeat(12) : id }, metageneration: '1', softDeletePolicy: { retentionDurationSeconds: '0' } };
    }
    if (url.includes('/o?')) return { items: url.includes('softDeleted=true') ? [{ hardDeleteTime: '2026-12-01T00:00:00Z' }] : [] };
    if (method === 'POST') { state = 'DESTROY_SCHEDULED'; return { name: version, state }; }
    if (url.endsWith('/cryptoKeyVersions')) return { cryptoKeyVersions: [{ name: version, state, destroyTime: '2026-12-24T00:00:00Z' }] };
    if (url.endsWith('/cryptoKeys')) return { cryptoKeys: [{ name: key, purpose: 'ENCRYPT_DECRYPT', labels: { 'momso-install-id': id } }] };
    if (url.endsWith('/cryptoKeys/backup')) return { name: key, purpose: 'ENCRYPT_DECRYPT', labels: { 'momso-install-id': id } };
    throw new Error('unexpected fixture request');
  };
  return { calls, request };
}

test('owned backup schedules destruction and reports both retention dates', async () => {
  const f = fixture();
  const value = await discardBackup({ projectId, installationId: id, request: f.request, sleep: async () => {} });
  assert.equal(value.resources.bucket.reason, 'soft_delete_retention');
  assert.equal(value.resources.kms.reason, 'destruction_scheduled');
  assert.deepEqual(value.billable.map(x => x.endsAt).sort(), ['2026-12-01T00:00:00Z', '2026-12-24T00:00:00Z']);
  assert.equal(value.billingZero, null);
});

test('foreign bucket stops before any changes', async () => {
  const f = fixture({ foreign: true });
  const value = await discardBackup({ projectId, installationId: id, request: f.request });
  assert.equal(value.resources.bucket.reason, 'ownership_unconfirmed');
  assert(!f.calls.some(x => x.method !== 'GET'));
});

test('retry after a missing bucket still schedules its labelled key', async () => {
  const f = fixture({ missingBucket: true });
  const value = await discardBackup({ projectId, installationId: id, request: f.request });
  assert.equal(value.resources.bucket.status, 'unverified');
  assert.equal(value.resources.kms.reason, 'destruction_scheduled');
});

test('retry retains the previously observed soft-deletion deadline', async () => {
  const f = fixture({ missingBucket: true });
  const value = await discardBackup({ projectId, installationId: id, request: f.request,
    previous: { billable: [{ resource: 'bucket', endsAt: '2099-12-01T00:00:00Z' }] } });
  assert.equal(value.resources.bucket.reason, 'soft_delete_retention');
  assert(value.billable.some(x => x.resource === 'bucket' && x.endsAt === '2099-12-01T00:00:00Z'));
});

test('foreign key is not changed', async () => {
  const f = fixture(), original = f.request;
  const request = async (method, url, body) => {
    const value = await original(method, url, body);
    if (method === 'GET' && url.endsWith('/cryptoKeys/backup')) value.labels['momso-install-id'] = 'b'.repeat(12);
    return value;
  };
  const value = await discardBackup({ projectId, installationId: id, request });
  assert.equal(value.resources.kms.reason, 'ownership_unconfirmed');
  assert(!f.calls.some(x => x.method !== 'GET'));
});

test('owned vault is removed only after backup ownership is verified', async () => {
  const f = fixture(), original = f.request; let deleted = false;
  const request = async (method, raw, body) => {
    if (raw.startsWith('https://secretmanager.googleapis.com/')) {
      if (method === 'DELETE') { assert(raw.includes('etag=fixture')); deleted = true; return {}; }
      return deleted ? null : { name: `projects/${projectId}/secrets/momso-vault-${id}`, labels: { 'momso-install-id': id }, etag: 'fixture' };
    }
    return original(method, raw, body);
  };
  const value = await discardBackup({ projectId, installationId: id, request, sleep: async () => {} });
  assert.equal(value.resources.secret_vault.status, 'deleted');
});

test('owned vault may use the numeric project resource name', async () => {
  const f = fixture(), original = f.request; let deleted = false;
  const request = async (method, raw, body) => {
    if (raw.startsWith('https://secretmanager.googleapis.com/')) {
      if (method === 'DELETE') { deleted = true; return {}; }
      return deleted ? null : { name: `projects/123456789/secrets/momso-vault-${id}`, labels: { 'momso-install-id': id }, etag: 'fixture' };
    }
    return original(method, raw, body);
  };
  const value = await discardBackup({ projectId, installationId: id, request, sleep: async () => {} });
  assert.equal(value.resources.secret_vault.status, 'deleted');
});

test('foreign vault is never removed', async () => {
  const f = fixture(), original = f.request; let deleted = false;
  const request = async (method, raw, body) => {
    if (raw.startsWith('https://secretmanager.googleapis.com/')) {
      if (method === 'DELETE') deleted = true;
      return { name: `projects/${projectId}/secrets/momso-vault-${id}`, labels: { 'momso-install-id': 'b'.repeat(12) }, etag: 'fixture' };
    }
    return original(method, raw, body);
  };
  const value = await discardBackup({ projectId, installationId: id, request, sleep: async () => {} });
  assert.equal(value.resources.secret_vault.status, 'unverified');
  assert.equal(deleted, false);
});

test('numeric project names in secret listings do not falsely prove absence', async () => {
  const f = fixture(), original = f.request;
  const request = async (method, raw, body) => {
    if (raw.startsWith('https://secretmanager.googleapis.com/')) {
      if (raw.endsWith('/secrets')) return { secrets: [{ name: `projects/123456789/secrets/momso-vault-${id}` }] };
      return null;
    }
    return original(method, raw, body);
  };
  const value = await discardBackup({ projectId, installationId: id, request, sleep: async () => {} });
  assert.equal(value.resources.secret_vault.status, 'unverified');
});

test('a later retry removes destroyed versions, the owned key, and its empty ring', async () => {
  let versions = [{ name: version, state: 'DESTROYED' }], keyExists = true, ringExists = true;
  const request = async (method, raw) => {
    const url = new URL(raw);
    if (url.hostname === 'storage.googleapis.com') {
      if (url.pathname === '/storage/v1/b') return { items: [] };
      return null;
    }
    if (url.pathname.includes('/operations/')) return { done: true };
    if (method === 'DELETE') {
      if (url.pathname.endsWith('/cryptoKeyVersions/1')) versions = [];
      else if (url.pathname.endsWith('/cryptoKeys/backup')) keyExists = false;
      else if (url.pathname.endsWith(`/keyRings/momso-backup-${id}`)) ringExists = false;
      else throw new Error('unexpected delete');
      return { name: `projects/${projectId}/locations/asia-northeast3/operations/fixture` };
    }
    if (url.pathname.endsWith('/cryptoKeyVersions')) return { cryptoKeyVersions: versions };
    if (url.pathname.endsWith('/cryptoKeys/backup')) return keyExists ? { name: key, purpose: 'ENCRYPT_DECRYPT', labels: { 'momso-install-id': id } } : null;
    if (url.pathname.endsWith('/cryptoKeys')) return { cryptoKeys: keyExists ? [{ name: key }] : [] };
    throw new Error('unexpected request');
  };
  const value = await discardBackup({ projectId, installationId: id, request, sleep: async () => {} });
  assert.equal(value.resources.bucket.status, 'deleted');
  assert.equal(value.resources.kms.status, 'deleted');
  assert.equal(ringExists, false);
  assert.equal(value.billingZero, null); // The other project resources have not been inventoried.
});
