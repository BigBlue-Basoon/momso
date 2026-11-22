import assert from 'node:assert/strict';
import { webcrypto } from 'node:crypto';
import test from 'node:test';
import { findFromGesture, startFromGesture } from './momso_anywhere_start.mjs';

test('resume uses the same installation without persisting account or tokens', async () => {
  const memory = new Map();
  const storage = { getItem: key => memory.get(key), setItem: (key, value) => memory.set(key, value) };
  const ids = [];
  const names = ['project', 'billing', 'services', 'privateNetwork', 'server', 'gateway', 'gatewayReady'];
  const provider = { preflight: async () => {}, gatewayUrl: () => 'https://fixture.a.run.app', ...Object.fromEntries(names.map(name => [name, async id => { if (name === 'project') ids.push(id); }])) };
  const authorize = async () => ({ provider, ownerSub: 'fixture-subject' });
  const options = { clientId: 'fixture-client', storage, cryptoImpl: webcrypto, authorize };
  assert.deepEqual(await startFromGesture(options), { url: 'https://fixture.a.run.app' });
  await startFromGesture(options);
  assert.equal(ids[0], ids[1]);
  assert(!JSON.stringify([...memory]).includes('fixture-subject'));
});

test('no resource changes occur when a release is not ready', async () => {
  let touched = false;
  const provider = { preflight: async () => { throw new Error('fixture'); }, project: async () => { touched = true; } };
  await assert.rejects(startFromGesture({ clientId: 'fixture', storage: {}, cryptoImpl: webcrypto, authorize: async () => ({ provider, ownerSub: 'fixture' }) }));
  assert.equal(touched, false);
});

test('address recovery signs in and returns discovered addresses without storing credentials', async () => {
  const result = await findFromGesture({ clientId: 'fixture', authorize: async () => ({ provider: { findExisting: async () => ['https://fixture.a.run.app'] } }) });
  assert.deepEqual(result, ['https://fixture.a.run.app']);
});
