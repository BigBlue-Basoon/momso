import test from 'node:test';
import assert from 'node:assert/strict';
import { webcrypto } from 'node:crypto';
import { GoogleBrowserProvider } from './momso_anywhere_google_browser.mjs';
import { discardBackup } from './momso_anywhere_discard_backup.mjs';
import { clearStartAuthorization, destroyFromGesture, resumeFromRedirect } from './momso_anywhere_start.mjs';

const projectId = 'fixture-project', id = 'a'.repeat(24), short = id.slice(0, 12);
const ring = `projects/${projectId}/locations/asia-northeast3/keyRings/momso-backup-${short}`;
const key = `${ring}/cryptoKeys/backup`, version = `${key}/cryptoKeyVersions/1`;
const reply = (value, status = 200) => ({ ok: status >= 200 && status < 300, status, json: async () => value });

function fakeCloud({ foreign = false, vmExists = false } = {}) {
  const calls = []; let state = 'ENABLED', vault = true;
  const fetchImpl = async (raw, options = {}) => {
    const url = new URL(raw), method = options.method ?? 'GET';
    calls.push({ url: url.href, method });
    assert.equal(options.headers.Authorization, 'Bearer fixture-token');
    if (url.hostname === 'cloudresourcemanager.googleapis.com') return reply({ projectId, state: 'ACTIVE' });
    if (url.hostname === 'compute.googleapis.com') return vmExists ? reply({ name: `momso-${short}` }) : reply({}, 404);
    if (url.hostname === 'storage.googleapis.com') {
      if (url.pathname === '/storage/v1/b') return reply({ items: url.searchParams.has('softDeleted') ? [] : [{ name: `momso-backup-${short}`, labels: { 'momso-install-id': id } }] });
      if (url.pathname.endsWith('/o')) return reply({ items: url.searchParams.has('softDeleted') ? [{ hardDeleteTime: '2099-12-01T00:00:00Z' }] : [] });
      return reply({ name: `momso-backup-${short}`, labels: { 'momso-install-id': foreign ? 'b'.repeat(24) : id }, metageneration: '1', softDeletePolicy: { retentionDurationSeconds: '0' } });
    }
    if (url.hostname === 'cloudkms.googleapis.com') {
      if (method === 'POST') { state = 'DESTROY_SCHEDULED'; return reply({ name: version, state }); }
      if (url.pathname.endsWith('/cryptoKeyVersions')) return reply({ cryptoKeyVersions: [{ name: version, state, destroyTime: '2099-12-24T00:00:00Z' }] });
      if (url.pathname.endsWith('/cryptoKeys')) return reply({ cryptoKeys: [{ name: key, purpose: 'ENCRYPT_DECRYPT', labels: { 'momso-install-id': id } }] });
      return reply({ name: key, purpose: 'ENCRYPT_DECRYPT', labels: { 'momso-install-id': id } });
    }
    if (url.hostname === 'secretmanager.googleapis.com') {
      if (method === 'DELETE') { vault = false; return reply({}); }
      return vault ? reply({ name: `projects/${projectId}/secrets/momso-vault-${short}`, labels: { 'momso-install-id': id }, etag: 'fixture' }) : reply({}, 404);
    }
    throw new Error('unexpected request');
  };
  return { calls, provider: new GoogleBrowserProvider('fixture-token', { projectId, fetchImpl, sleep: async () => {} }) };
}

test('browser REST cleanup checks VM and returns PC-shaped retention result', async () => {
  const f = fakeCloud(), result = await f.provider.discardBackup(id);
  assert.equal(result.mode, 'delete_all');
  assert.equal(result.resources.bucket.reason, 'soft_delete_retention');
  assert.equal(result.resources.kms.reason, 'destruction_scheduled');
  assert.equal(result.resources.secret_vault.status, 'deleted');
  assert.equal(result.backupChargesEndAt, '2099-12-24T00:00:00Z');
  assert.equal(result.billingZero, null);
  assert(f.calls.findIndex(x => x.url.includes('/instances/')) < f.calls.findIndex(x => x.method === 'POST'));
});

test('browser REST never changes a foreign backup or a running VM', async () => {
  for (const options of [{ foreign: true }, { vmExists: true }]) {
    const f = fakeCloud(options);
    if (options.vmExists) await assert.rejects(f.provider.discardBackup(id), { step: 'vm_unconfirmed' });
    else assert.equal((await f.provider.discardBackup(id)).resources.bucket.reason, 'ownership_unconfirmed');
    assert(!f.calls.some(x => x.method !== 'GET'));
  }
});

test('backup deletion never creates or discovers an unselected project', async () => {
  let called = false;
  const provider = new GoogleBrowserProvider('fixture-token', { fetchImpl: async () => { called = true; throw new Error('unexpected'); } });
  await assert.rejects(provider.discardBackup(id), { step: 'project_required' });
  assert.equal(called, false);
});

test('a temporary backup lookup failure keeps earlier billing deadlines', async () => {
  const previous = { billable: [{ resource: 'bucket', endsAt: '2099-12-01T00:00:00Z' }] };
  const fetchImpl = async raw => {
    const url = new URL(raw);
    if (url.hostname === 'cloudresourcemanager.googleapis.com') return reply({ projectId, state: 'ACTIVE' });
    if (url.hostname === 'compute.googleapis.com') return reply({}, 404);
    return reply({}, 503);
  };
  const provider = new GoogleBrowserProvider('fixture-token', { projectId, fetchImpl, sleep: async () => {} });
  const result = await provider.discardBackup(id, previous);
  assert.equal(result.resources.bucket.status, 'unverified');
  assert.equal(result.billable[0].endsAt, '2099-12-01T00:00:00Z');
  assert.equal(result.billingZero, null);
});

test('a later KMS lookup failure retains the previous key deadline', async () => {
  let reads = 0;
  const request = async (_method, raw) => {
    if (raw.includes('storage.googleapis.com/storage/v1/b/')) return null;
    if (raw.includes('storage.googleapis.com/storage/v1/b?')) return { items: [] };
    if (raw.endsWith('/cryptoKeyVersions')) {
      if (++reads > 1) throw new Error('temporarily unavailable');
      return { cryptoKeyVersions: [{ name: version, state: 'DESTROY_SCHEDULED' }] };
    }
    if (raw.endsWith('/cryptoKeys/backup')) return { name: key, purpose: 'ENCRYPT_DECRYPT', labels: { 'momso-install-id': id } };
    throw new Error('unexpected');
  };
  const result = await discardBackup({ projectId, installationId: id, request,
    previous: { billable: [{ resource: 'kms', endsAt: '2099-12-24T00:00:00Z' }] } });
  assert.equal(result.resources.kms.reason, 'cleanup_unverified');
  assert(result.billable.some(item => item.resource === 'kms' && item.endsAt === '2099-12-24T00:00:00Z'));
});

test('gesture retry retains previous deadlines without storing access tokens', async () => {
  const mem = () => { const values = new Map(); return { values, getItem: k => values.get(k) ?? null, setItem: (k, v) => values.set(k, v), removeItem: k => values.delete(k) }; };
  const session = mem(), storage = mem(), previous = [], clientId = 'fixture.apps.googleusercontent.com';
  const location = { href: 'https://fixture.example/start/', origin: 'https://fixture.example', assign(value) { this.next = value; } };
  const provider = { async destroy() { return { remaining: [] }; }, async discardBackup(_id, old) {
    previous.push(old);
    return { mode: 'delete_all', resources: { bucket: { status: 'retained', reason: 'soft_delete_retention' }, kms: { status: 'retained', reason: 'destruction_scheduled' }, secret_vault: { status: 'deleted' } }, billable: [{ resource: 'bucket', endsAt: '2099-12-01T00:00:00Z' }], billingZero: null };
  } };
  const options = { clientId, release: {}, projectId, installationId: id, discardData: true, session, storage, location, cryptoImpl: webcrypto, providerFactory: async () => ({ provider, ownerSub: '12345' }) };
  clearStartAuthorization();
  await destroyFromGesture(options);
  const state = new URL(location.next).searchParams.get('state');
  const fragment = new URLSearchParams({ state, access_token: 'fixture-token', token_type: 'Bearer', expires_in: '60', scope: 'openid email https://www.googleapis.com/auth/cloud-platform' }).toString();
  const first = await resumeFromRedirect({ ...options, fragment });
  assert.equal(first.value.status, 'pending');
  const second = await destroyFromGesture(options);
  assert.equal(second.status, 'pending');
  assert.equal(previous[1].billable[0].endsAt, '2099-12-01T00:00:00Z');
  assert(!JSON.stringify([...storage.values]).includes('fixture-token'));
  assert(!JSON.stringify([...session.values]).includes('fixture-token'));
  clearStartAuthorization();
});

test('failed VM removal never calls backup deletion', async () => {
  const mem = () => { const values = new Map(); return { getItem: k => values.get(k) ?? null, setItem: (k, v) => values.set(k, v), removeItem: k => values.delete(k) }; };
  const session = mem(), storage = mem(), clientId = 'fixture.apps.googleusercontent.com';
  const location = { href: 'https://fixture.example/start/', origin: 'https://fixture.example', assign(value) { this.next = value; } };
  let cleaned = false;
  const provider = { async destroy() { throw new Error('fixture'); }, async discardBackup() { cleaned = true; } };
  const options = { clientId, release: {}, projectId, installationId: id, discardData: true, session, storage, location, cryptoImpl: webcrypto, providerFactory: async () => ({ provider, ownerSub: '12345' }) };
  storage.setItem(`momso.destroy.${projectId}.${id}`, JSON.stringify({ projectId, installationId: id, deletion: { billable: [{ resource: 'kms', endsAt: '2030-01-01T00:00:00Z' }] } }));
  clearStartAuthorization();
  await destroyFromGesture(options);
  const state = new URL(location.next).searchParams.get('state');
  const fragment = new URLSearchParams({ state, access_token: 'fixture-token', token_type: 'Bearer', expires_in: '60', scope: 'openid email https://www.googleapis.com/auth/cloud-platform' }).toString();
  const result = (await resumeFromRedirect({ ...options, fragment })).value;
  assert.equal(result.outcome, 'failed');
  assert.deepEqual(result.billable, [{ resource: 'kms', endsAt: '2030-01-01T00:00:00Z' }]);
  assert.equal(cleaned, false);
  clearStartAuthorization();
});
