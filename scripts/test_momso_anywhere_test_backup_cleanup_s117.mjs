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
    if (url.includes('/o?')) return { items: url.includes('softDeleted=true') ? [] : [{ name: 'fictional.txt', generation: '123' }] };
    return { name: `momso-backup-${short}`, labels: { 'momso-install-id': bucketId }, metageneration: '2', softDeletePolicy: { retentionDurationSeconds: '0' } };
  };
  return { calls, options: { projectId, installationId: id, protectedIds: ['b'.repeat(12)], verifyFresh: async () => fresh, request, sleep: async () => {} }, foreign: () => { bucketId = 'c'.repeat(24); }, notFresh: () => { fresh = false; } };
}

test('owned test bucket cleanup schedules keys and reports potential cost', async () => {
  const f = fixture(), result = await cleanupTestBackup(f.options);
  assert.deepEqual(result.keyVersions, [{ state: 'DESTROY_SCHEDULED', billing: 'may_be_billable' }]);
  assert(f.calls.some(call => call.method === 'DELETE' && call.url.includes('ifGenerationMatch=123')));
  assert(f.calls.some(call => call.method === 'DELETE' && call.url.endsWith('ifMetagenerationMatch=2')));
});

test('disables soft deletion and waits before deleting live objects', async () => {
  const f = fixture(), original = f.options.request, events = [];
  f.options.sleep = async ms => { events.push(['wait', ms]); };
  f.options.request = async (method, url, body) => {
    events.push([method, url]);
    if (method === 'PATCH') {
      assert.equal(body.softDeletePolicy.retentionDurationSeconds, '0');
      return { name: `momso-backup-${short}`, labels: { 'momso-install-id': id }, metageneration: '3', softDeletePolicy: { retentionDurationSeconds: '0' } };
    }
    if (method === 'GET' && url.includes('softDeleted=true')) return { items: [{ hardDeleteTime: '2026-12-01T00:00:00Z' }] };
    const value = await original(method, url);
    if (method === 'GET' && value?.metageneration) value.softDeletePolicy.retentionDurationSeconds = '604800';
    return value;
  };
  const result = await cleanupTestBackup(f.options);
  assert.deepEqual(result.softDeleted, [{ hardDeleteTime: '2026-12-01T00:00:00Z' }]);
  assert(events.findIndex(e => e[0] === 'wait' && e[1] === 30000) < events.findIndex(e => e[0] === 'DELETE'));
  assert.equal(result.bucket, 'retained_soft_deleted');
  assert(!events.some(e => e[0] === 'DELETE' && e[1].endsWith('ifMetagenerationMatch=3')));
});

test('does not delete anything if the policy update cannot be verified', async () => {
  const f = fixture(), original = f.options.request;
  f.options.request = async (method, url) => {
    if (method === 'PATCH') return { name: `momso-backup-${short}`, labels: { 'momso-install-id': 'c'.repeat(24) }, metageneration: '3', softDeletePolicy: { retentionDurationSeconds: '0' } };
    const result = await original(method, url);
    if (method === 'GET' && result?.metageneration) result.softDeletePolicy.retentionDurationSeconds = '604800';
    return result;
  };
  await assert.rejects(cleanupTestBackup(f.options), /policy_change_unconfirmed/);
  assert(!f.calls.some(call => call.method === 'POST' || call.method === 'DELETE'));
});

test('retries wait for propagation even if the policy already reads zero', async () => {
  const f = fixture(), waits = [];
  f.options.sleep = async ms => waits.push(ms);
  await cleanupTestBackup(f.options);
  assert.deepEqual(waits, [30000]);
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
