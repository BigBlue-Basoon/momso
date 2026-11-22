import { test } from 'node:test';
import assert from 'node:assert/strict';
import { prepareBackupResources } from './momso_anywhere_backup_setup.mjs';

test('backup resources preserve existing policy and use the VM identity', async () => {
  const id = '0123456789abcdef01234567';
  const short = id.slice(0, 12);
  const project = 'fixture-project';
  const bucket = `momso-backup-${short}`;
  const ring = `projects/${project}/locations/asia-northeast3/keyRings/${bucket}`;
  const key = `${ring}/cryptoKeys/backup`;
  const resources = new Map();
  const writes = [];
  const request = async (url, options = {}) => {
    if (options.method) {
      writes.push({ url, ...options });
      if (url.includes('/storage/v1/b?')) resources.set(`https://storage.googleapis.com/storage/v1/b/${bucket}`, { ...options.body });
      if (url.includes('/keyRings?keyRingId=')) resources.set(`https://cloudkms.googleapis.com/v1/${ring}`, { name: ring });
      if (url.includes('/cryptoKeys?cryptoKeyId=')) resources.set(`https://cloudkms.googleapis.com/v1/${key}`, { ...options.body, name: key });
      return {};
    }
    if (url.includes('/iam?') || url.includes(':getIamPolicy?')) return { etag: 'fixture', bindings: [{ role: 'roles/viewer', members: ['user:kept@example.invalid'] }] };
    return resources.get(url) ?? null;
  };
  assert.deepEqual(await prepareBackupResources(request, project, id), { bucket, key });
  const policies = writes.filter(call => call.url.endsWith('/iam') || call.url.endsWith(':setIamPolicy'));
  assert.equal(policies.length, 2);
  for (const call of policies) {
    const policy = call.body.policy ?? call.body;
    assert(policy.bindings.some(value => value.members.includes('user:kept@example.invalid')));
    assert(policy.bindings.some(value => value.members.includes(`serviceAccount:momso-vm-${short}@${project}.iam.gserviceaccount.com`)));
  }
  assert(!writes.some(call => call.method === 'DELETE'));
});

test('foreign bucket is rejected before changing its policy', async () => {
  let changes = 0;
  await assert.rejects(prepareBackupResources(async (_url, options = {}) => {
    if (options.method) changes++;
    return { name: 'other', labels: {} };
  }, 'fixture-project', '0123456789abcdef01234567'));
  assert.equal(changes, 0);
});

test('foreign encryption key is rejected before changing its policy', async () => {
  const id = '0123456789abcdef01234567', project = 'fixture-project', short = id.slice(0, 12);
  const bucket = `momso-backup-${short}`;
  const ring = `projects/${project}/locations/asia-northeast3/keyRings/${bucket}`;
  let changes = 0;
  await assert.rejects(prepareBackupResources(async (url, options = {}) => {
    if (options.method) changes++;
    if (url.includes('/storage/')) return { name: bucket, labels: { 'momso-install-id': id }, iamConfiguration: { uniformBucketLevelAccess: { enabled: true }, publicAccessPrevention: 'enforced' } };
    if (url.endsWith('/cryptoKeys/backup')) return { name: `${ring}/cryptoKeys/backup`, purpose: 'ENCRYPT_DECRYPT', labels: { 'momso-install-id': 'someone-else' } };
    return { name: ring };
  }, project, id));
  assert.equal(changes, 0);
});
