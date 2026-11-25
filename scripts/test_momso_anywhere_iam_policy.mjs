import test from 'node:test';
import assert from 'node:assert/strict';
import { withoutAccounts, removeAndVerify } from './momso_anywhere_iam_policy.mjs';

test('removes exact live and deleted accounts while keeping other principals and conditions', () => {
  const before = { etag: 'fixture', version: 3, bindings: [{ role: 'r', condition: { title: 'keep' }, members: ['serviceAccount:fixture@example.test', 'deleted:serviceAccount:fixture@example.test?uid=123', 'user:other@example.test'] }] };
  const after = withoutAccounts(before, ['fixture@example.test']);
  assert.deepEqual(after.bindings[0].members, ['user:other@example.test']);
  assert.equal(after.bindings[0].condition.title, 'keep');
  assert.equal(before.bindings[0].members.length, 3);
});

test('writes and rereads policy before allowing deletion', async () => {
  let policy = { etag: 'e', bindings: [{ role: 'r', members: ['serviceAccount:fixture@example.test'] }] };
  const steps = [];
  await removeAndVerify({ resources: ['fixture'], emails: ['fixture@example.test'], read: async () => { steps.push('read'); return structuredClone(policy); }, write: async (_resource, value) => { steps.push('write'); policy = value; } });
  steps.push('delete-account');
  assert.deepEqual(steps, ['read', 'write', 'read', 'delete-account']);
});

test('stale deleted member or missing etag stops cleanup', async () => {
  const policy = { etag: 'e', bindings: [{ role: 'r', members: ['deleted:serviceAccount:fixture@example.test?uid=123'] }] };
  await assert.rejects(removeAndVerify({ resources: ['fixture'], emails: ['fixture@example.test'], read: async () => policy, write: async () => {} }), /iam_bindings_unverified/);
  assert.throws(() => withoutAccounts({}, []), /iam_bindings_unverified/);
});
