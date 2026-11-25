import assert from 'node:assert/strict';
import test from 'node:test';
import { googleBillingPreflight, startAnywhere, SetupError } from './momso_anywhere_setup.mjs';

const names = ['project', 'billing', 'services', 'privateNetwork', 'server', 'gateway', 'gatewayReady'];
const checkpoint = { installationId: 'a'.repeat(24) };

test('reconciles an interrupted installation and never stores a token', async () => {
  const resources = new Set();
  const calls = [];
  let interrupted = true;
  const provider = Object.fromEntries(names.map((name) => [name, async (id) => {
    assert.equal(id, checkpoint.installationId);
    calls.push(name);
    if (name === 'server' && interrupted) {
      interrupted = false;
      throw new Error('fixture');
    }
    resources.add(name);
  }]));
  await assert.rejects(startAnywhere(provider, checkpoint), { step: 'server' });
  await startAnywhere(provider, checkpoint);
  assert.deepEqual([...resources], names);
  assert.equal(calls.filter((name) => name === 'project').length, 2);
  assert.deepEqual(Object.keys(checkpoint), ['installationId']);
});

test('an incomplete provider is rejected before creating anything', async () => {
  let invoked = false;
  await assert.rejects(startAnywhere({ project: async () => { invoked = true; } }, checkpoint), { step: 'provider_not_ready' });
  assert.equal(invoked, false);
});

test('preserved assets stop installation before network and VM creation', async () => {
  const calls = [];
  const provider = Object.fromEntries(names.map(name => [name, async () => { calls.push(name); }]));
  provider.restorePlan = async () => ({ restoreAvailable: true, requiresRestore: true });
  await assert.rejects(startAnywhere(provider, checkpoint), { step: 'restore_contract_pending' });
  assert.deepEqual(calls, ['project', 'billing', 'services']);
});

test('restore preparation runs before VM and reports the verified result', async () => {
  const calls = [];
  const provider = Object.fromEntries(names.map(name => [name, async () => { calls.push(name); }]));
  provider.restorePlan = async () => ({ restoreAvailable: true, lastBackupAt: '2026-11-20T00:00:00Z', requiresRestore: true });
  provider.enableRestore = (id, plan) => { assert.equal(id, checkpoint.installationId); assert.equal(plan.requiresRestore, true); calls.push('restore'); };
  provider.restoreResult = () => ({ restored: true, restoredBackupId: 'slot-0@1', restoredAt: '2026-11-20T00:00:00Z' });
  const result = await startAnywhere(provider, checkpoint);
  assert(calls.indexOf('restore') < calls.indexOf('server'));
  assert.equal(result.restoredBackupId, 'slot-0@1');
});

test('failed restore stops before gateway and preserves a safe detail code', async () => {
  const calls = [];
  const provider = Object.fromEntries(names.map(name => [name, async () => { calls.push(name); if (name === 'server') { const error = new SetupError('restore_failed'); error.detail = 'key_unavailable'; throw error; } }]));
  provider.restorePlan = async () => ({ requiresRestore: true });
  provider.enableRestore = () => {};
  await assert.rejects(startAnywhere(provider, checkpoint), error => error.step === 'restore_failed' && error.detail === 'key_unavailable');
  assert(!calls.includes('gateway'));
});

test('billing preflight calls only the selected project and does not return its token', async () => {
  const result = await googleBillingPreflight('fixture-project', 'fixture-secret', async (url, options) => {
    assert.equal(url, 'https://cloudbilling.googleapis.com/v1/projects/fixture-project/billingInfo');
    assert.equal(options.headers.Authorization, 'Bearer fixture-secret');
    assert.equal(options.redirect, 'error');
    return { ok: true, json: async () => ({ billingEnabled: true }) };
  });
  assert.deepEqual(result, { billingEnabled: true });
});
