import assert from 'node:assert/strict';
import test from 'node:test';
import { cleanupTestBackup } from './momso_anywhere_test_backup_cleanup_s117.mjs';

const id = 'a'.repeat(24), projectId = 'fixture-project', short = id.slice(0, 12);
const ring = `projects/${projectId}/locations/asia-northeast3/keyRings/momso-backup-${short}`;
const version = `${ring}/cryptoKeys/backup/cryptoKeyVersions/1`;
function fixture() {
  const calls = []; let bucketId = id, fresh = true;
  const request = async (method, url) => {
    calls.push({ method, url });
    if (method === 'POST') return { name: version, state: 'DESTROY_SCHEDULED' };
    if (method === 'DELETE') return {};
    if (url.endsWith('/cryptoKeyVersions')) return { cryptoKeyVersions: [{ name: version, state: 'ENABLED' }] };
    if (url.endsWith('/cryptoKeys')) return { cryptoKeys: [{ name: `${ring}/cryptoKeys/backup`, labels: { 'momso-install-id': bucketId }, purpose: 'ENCRYPT_DECRYPT' }] };
    if (url.includes('/o?')) return { items: [{ name: 'fictional.txt', generation: '123' }] };
    return { name: `momso-backup-${short}`, labels: { 'momso-install-id': bucketId }, metageneration: '2' };
  };
  return { calls, options: { projectId, installationId: id, protectedIds: ['b'.repeat(12)], verifyFresh: async () => fresh, request }, foreign: () => { bucketId = 'c'.repeat(24); }, notFresh: () => { fresh = false; } };
}

test('owned test bucket cleanup schedules keys and reports potential cost', async () => {
  const f = fixture(), result = await cleanupTestBackup(f.options);
  assert.deepEqual(result.keyVersions, [{ state: 'DESTROY_SCHEDULED', billing: 'may_be_billable' }]);
  assert(f.calls.some(call => call.method === 'DELETE' && call.url.includes('ifGenerationMatch=123')));
  assert(f.calls.some(call => call.method === 'DELETE' && call.url.endsWith('ifMetagenerationMatch=2')));
});

for (const mode of ['foreign', 'notFresh', 'protected']) test(`rejects ${mode} before changes`, async () => {
  const f = fixture();
  if (mode === 'protected') f.options.protectedIds = [id.slice(0, 12)]; else f[mode]();
  await assert.rejects(cleanupTestBackup(f.options));
  assert(!f.calls.some(call => call.method === 'POST' || call.method === 'DELETE'));
});

test('unexpected keys fail closed before deletion', async () => {
  const f = fixture(), original = f.options.request;
  f.options.request = async (method, url) => url.endsWith('/cryptoKeys') ? { cryptoKeys: [{ name: `${ring}/cryptoKeys/foreign` }] } : original(method, url);
  await assert.rejects(cleanupTestBackup(f.options), /keys_unconfirmed/);
  assert(!f.calls.some(call => call.method === 'POST' || call.method === 'DELETE'));
});

test('key without matching ownership label is never destroyed', async () => {
  const f = fixture(), original = f.options.request;
  f.options.request = async (method, url) => {
    const result = await original(method, url);
    if (result?.cryptoKeys) result.cryptoKeys[0].labels['momso-install-id'] = 'c'.repeat(24);
    return result;
  };
  await assert.rejects(cleanupTestBackup(f.options), /keys_unconfirmed/);
  assert(!f.calls.some(call => call.method === 'POST' || call.method === 'DELETE'));
});

test('a 12-character PC installation still checks the full recorded label', async () => {
  const f = fixture(), original = f.options.request;
  f.options.installationId = short;
  f.options.request = async (method, url) => {
    const result = await original(method, url);
    if (method === 'GET' && result?.labels) result.labels['momso-install-id'] = short;
    if (result?.cryptoKeys) result.cryptoKeys[0].labels['momso-install-id'] = short;
    return result;
  };
  assert.equal((await cleanupTestBackup(f.options)).bucket, 'delete_requested');
});
