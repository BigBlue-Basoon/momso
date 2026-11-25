import assert from 'node:assert/strict';
import { webcrypto } from 'node:crypto';
import test from 'node:test';
import { clearStartAuthorization, findFromGesture, openGoogleSetup, resumeAfterRedirect, startFromGesture } from './momso_anywhere_start.mjs';

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

test('Google setup opens a separate tab and keeps the saved action', async () => {
  clearStartAuthorization();
  const memory = new Map([['momso.start.retry.v1', JSON.stringify({ action: 'billing', projectId: 'fixture-project', billingAccountName: null, installationId: null })]]);
  const session = { getItem: k => memory.get(k) ?? null, setItem: (k, v) => memory.set(k, v), removeItem: k => memory.delete(k) };
  const location = { origin: 'https://start.example.test', href: 'https://start.example.test/start/', assign(value) { this.assigned = value; } };
  let opened;
  openGoogleSetup({ destination: 'billing', session, open: (...args) => { opened = args; } });
  assert.deepEqual(opened, ['https://console.cloud.google.com/billing', '_blank', 'noopener,noreferrer']);
  assert.equal(JSON.parse(memory.get('momso.start.retry.v1')).action, 'billing');
  assert(!JSON.stringify([...memory]).includes('access_token'));
  assert.equal(memory.has('momso.start.cloud-return.v1'), false);
});

test('Google setup accepts first entry and refuses arbitrary destinations', () => {
  const data = new Map();
  const session = { getItem: key => data.get(key), setItem: (key, value) => data.set(key, value) };
  const open = () => {};
  assert.throws(() => openGoogleSetup({ destination: 'https://example.test', session, open }), { step: 'invalid_destination' });
  openGoogleSetup({ destination: 'billing', session, open });
  assert.equal(JSON.parse(data.get('momso.start.retry.v1')).action, 'billing');
});
