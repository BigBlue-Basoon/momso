import assert from 'node:assert/strict';
import test from 'node:test';
import { googleBillingPreflight, startAnywhere } from './momso_anywhere_setup.mjs';

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

test('billing preflight calls only the selected project and does not return its token', async () => {
  const result = await googleBillingPreflight('fixture-project', 'fixture-secret', async (url, options) => {
    assert.equal(url, 'https://cloudbilling.googleapis.com/v1/projects/fixture-project/billingInfo');
    assert.equal(options.headers.Authorization, 'Bearer fixture-secret');
    assert.equal(options.redirect, 'error');
    return { ok: true, json: async () => ({ billingEnabled: true }) };
  });
  assert.deepEqual(result, { billingEnabled: true });
});
