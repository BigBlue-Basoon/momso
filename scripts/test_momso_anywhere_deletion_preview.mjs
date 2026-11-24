import assert from 'node:assert/strict';
import test from 'node:test';
import { webcrypto } from 'node:crypto';
import { GoogleBrowserProvider } from './momso_anywhere_google_browser.mjs';
import { clearStartAuthorization, deletionPreviewFromGesture, resumeFromRedirect } from './momso_anywhere_start.mjs';

const id = 'a'.repeat(24), short = id.slice(0, 12), projectId = 'fixture-project';
const ok = (data, status = 200) => ({ ok: status >= 200 && status < 300, status, json: async () => data });

function fake({ foreign = false, deny = false } = {}) {
  const calls = [];
  const provider = new GoogleBrowserProvider('fixture-token', { projectId, fetchImpl: async (raw, options) => {
    const url = new URL(raw); calls.push([url.pathname, options.method]);
    if (url.hostname === 'cloudresourcemanager.googleapis.com') return ok({ projectId, state: 'ACTIVE' });
    if (url.hostname === 'storage.googleapis.com' && !url.pathname.endsWith('/o')) return ok({ name: `momso-backup-${short}`, labels: { 'momso-install-id': foreign ? 'b'.repeat(24) : id } });
    if (url.hostname === 'storage.googleapis.com') {
      if (deny) return ok({}, 403);
      return ok(url.searchParams.has('softDeleted') ? {} : { items: [{ name: 'momso-backup/slot-2.enc', updated: '2026-11-24T00:00:00Z' }] });
    }
    if (url.pathname.endsWith('/cryptoKeyVersions')) return ok({ cryptoKeyVersions: [{ state: 'ENABLED', protectionLevel: 'SOFTWARE' }] });
    if (url.hostname === 'cloudkms.googleapis.com') return ok({ name: `projects/${projectId}/locations/asia-northeast3/keyRings/momso-backup-${short}/cryptoKeys/backup`, labels: { 'momso-install-id': id } });
    throw Error('unexpected');
  } });
  return { provider, calls };
}

test('preview returns backup time and estimated key cost without modifying resources', async () => {
  const f = fake(), result = await f.provider.deletionPreview(id);
  assert.equal(result.backupExists, true);
  assert.equal(result.lastBackupAt, '2026-11-24T00:00:00Z');
  assert.equal(result.estimatedMonthly.kms, 0.06);
  assert.equal(result.estimatedMonthly.total, null);
  assert(f.calls.every(([, method]) => method === 'GET'));
});

test('foreign bucket blocks preview and inaccessible listing stays unknown', async () => {
  await assert.rejects(fake({ foreign: true }).provider.deletionPreview(id), { step: 'resource_ownership' });
  const result = await fake({ deny: true }).provider.deletionPreview(id);
  assert.equal(result.backupExists, null);
});

test('preview is available through the existing gesture and redirect', async () => {
  clearStartAuthorization();
  const mem = () => { const m = new Map(); return { getItem: k => m.get(k) ?? null, setItem: (k, v) => m.set(k, v), removeItem: k => m.delete(k) }; };
  const session = mem(), storage = mem(), f = fake();
  const location = { href: 'https://fixture.test/start/', origin: 'https://fixture.test', assign(url) { this.next = url; } };
  const options = { projectId, installationId: id, clientId: 'fixture.apps.googleusercontent.com', release: {}, session, storage, location, cryptoImpl: webcrypto, providerFactory: async () => ({ provider: f.provider, ownerSub: '12345' }) };
  assert.deepEqual(await deletionPreviewFromGesture(options), { redirecting: true });
  const state = new URL(location.next).searchParams.get('state');
  const fragment = new URLSearchParams({ state, access_token: 'fixture-token', token_type: 'Bearer', expires_in: '60', scope: 'openid email https://www.googleapis.com/auth/cloud-platform' });
  const result = await resumeFromRedirect({ ...options, fragment: '#' + fragment });
  assert.equal(result.action, 'deletion_preview');
  assert.equal(result.value.backupExists, true);
  assert.equal(result.value.projectExclusivity.status, 'unverified');
  assert(f.calls.every(([, method]) => method === 'GET'));
  clearStartAuthorization();
});
