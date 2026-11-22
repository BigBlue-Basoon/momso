import assert from 'node:assert/strict';
import { webcrypto } from 'node:crypto';
import test from 'node:test';
import { bootLoginPage } from './momso_anywhere_login_boot.mjs';

const origin = 'https://fixture.run.app';
const makeState = iat => `${Buffer.from(JSON.stringify({ origin, iat })).toString('base64url')}.signature`;
const signed = makeState(Math.floor(Date.now() / 1000));
const storage = () => { const m = new Map(); return { getItem: k => m.get(k) ?? null, setItem: (k, v) => m.set(k, v), removeItem: k => m.delete(k) }; };
function fixture(href, referrer, memory = storage()) {
  const button = { disabled: false, addEventListener(_type, callback) { this.click = callback; } };
  const doc = { body: { dataset: {} }, referrer, getElementById: id => id === 'google-login' ? button : null };
  const win = { location: { href, origin: 'https://login.example.test', assign(value) { this.assigned = value; }, replace(value) { this.replaced = value; } }, sessionStorage: memory, history: { replaceState(_state, _title, path) { this.path = path; } } };
  return { doc, win, button };
}
const config = async () => ({ ok: true, json: async () => ({ clientId: 'fixture.apps.googleusercontent.com' }) });

test('missing state and mismatched origin become invalid and disable the button', async () => {
  for (const item of [fixture('https://login.example.test/', origin), fixture(`https://login.example.test/?state=${signed}`, 'https://other.run.app')]) {
    await bootLoginPage({ ...item, fetchImpl: async () => assert.fail('no request'), cryptoImpl: webcrypto });
    assert.equal(item.doc.body.dataset.state, 'invalid');
    assert.equal(item.button.disabled, true);
  }
});

test('button uses top-level OIDC redirect and consumes the return fragment', async () => {
  const memory = storage();
  const first = fixture(`https://login.example.test/?state=${signed}`, origin, memory);
  await bootLoginPage({ ...first, fetchImpl: config, cryptoImpl: webcrypto });
  assert.equal(first.doc.body.dataset.state, 'ready');
  assert.equal(first.button.disabled, false);
  await first.button.click();
  assert.equal(first.doc.body.dataset.state, 'working');
  assert.equal(first.button.disabled, true);
  const google = new URL(first.win.location.assigned);
  const returned = fixture(`https://login.example.test/#id_token=a.b.c&state=${google.searchParams.get('state')}`, 'https://accounts.google.com/', memory);
  await bootLoginPage({ ...returned, fetchImpl: async () => assert.fail('no request'), cryptoImpl: webcrypto });
  assert.equal(returned.win.history.path, '/');
  assert.equal(returned.doc.body.dataset.state, 'working');
  assert.equal(new URL(returned.win.location.replaced).origin, origin);
  assert(!returned.win.location.replaced.includes('?'));
});

test('mismatched callback state is removed from address and becomes error', async () => {
  const item = fixture('https://login.example.test/#id_token=a.b.c&state=unexpected', 'https://accounts.google.com/');
  await bootLoginPage({ ...item, fetchImpl: async () => assert.fail('no request'), cryptoImpl: webcrypto });
  assert.equal(item.win.history.path, '/');
  assert.equal(item.doc.body.dataset.state, 'error');
  assert.equal(item.button.disabled, false);
});

test('Google error permits retry with the saved challenge', async () => {
  const memory = storage();
  const first = fixture(`https://login.example.test/?state=${signed}`, origin, memory);
  await bootLoginPage({ ...first, fetchImpl: config, cryptoImpl: webcrypto });
  await first.button.click();
  const google = new URL(first.win.location.assigned);
  const denied = fixture(`https://login.example.test/#error=access_denied&state=${google.searchParams.get('state')}`, 'https://accounts.google.com/', memory);
  await bootLoginPage({ ...denied, fetchImpl: config, cryptoImpl: webcrypto });
  assert.equal(denied.win.history.path, '/');
  assert.equal(denied.doc.body.dataset.state, 'error');
  assert.equal(denied.button.disabled, false);
  await denied.button.click();
  assert.equal(new URL(denied.win.location.assigned).hostname, 'accounts.google.com');
});

test('old challenge retry returns to the original gateway', async () => {
  const old = makeState(Math.floor(Date.now() / 1000) - 250);
  const item = fixture(`https://login.example.test/?state=${old}&error=1`, origin);
  await bootLoginPage({ ...item, fetchImpl: config, cryptoImpl: webcrypto });
  assert.equal(item.doc.body.dataset.state, 'error');
  assert.equal(item.button.disabled, false);
  await item.button.click();
  assert.equal(item.win.location.replaced, origin + '/');
});
