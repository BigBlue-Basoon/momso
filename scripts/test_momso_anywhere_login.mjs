import assert from 'node:assert/strict';
import test from 'node:test';
import { webcrypto } from 'node:crypto';
import { authorizationUrl, consumeGoogleReturn, prepareLogin, validateRedirectContext } from './momso_anywhere_login.mjs';

const origin = 'https://fixture.run.app';
const state = `${Buffer.from(JSON.stringify({ origin, iat: 1000, random: 'fixture' })).toString('base64url')}.signature`;
const storage = () => {
  const values = new Map();
  return { getItem: key => values.get(key) ?? null, setItem: (key, value) => values.set(key, value), removeItem: key => values.delete(key) };
};

test('OIDC redirect uses nonce-bound signed state and retains identity in fragments only', async () => {
  const memory = storage();
  prepareLogin({ state, referrer: origin + '/path', storage: memory });
  const url = new URL(await authorizationUrl({ state, referrer: origin, clientId: 'fixture', pageOrigin: 'https://login.example.test', storage: memory, cryptoImpl: webcrypto }));
  assert.equal(url.origin, 'https://accounts.google.com');
  assert.equal(url.searchParams.get('response_type'), 'id_token');
  assert.equal(url.searchParams.get('response_mode'), 'fragment');
  assert.equal(url.searchParams.get('scope'), 'openid email');
  assert.equal(url.searchParams.get('redirect_uri'), 'https://login.example.test/');
  assert.equal(url.searchParams.get('nonce'), Buffer.from(await webcrypto.subtle.digest('SHA-256', new TextEncoder().encode(state))).toString('base64url'));
  const returned = new URL(consumeGoogleReturn({ fragment: `#id_token=a.b.c&state=${url.searchParams.get('state')}`, storage: memory }));
  assert.equal(returned.origin, origin);
  assert.equal(returned.pathname, '/__momso/auth/return');
  assert.equal(returned.search, '');
  assert.equal(new URLSearchParams(returned.hash.slice(1)).get('id_token'), 'a.b.c');
  assert.throws(() => consumeGoogleReturn({ fragment: '#id_token=a.b.c&state=bad', storage: memory }), /login_failed/);
});

test('referrer mismatch, unexpected Google state and malformed tokens are rejected', async () => {
  assert.equal(validateRedirectContext(state, '').origin, origin);
  assert.throws(() => validateRedirectContext(state, 'https://other.run.app/'), /invalid_login_origin/);
  assert.throws(() => validateRedirectContext('', origin), /invalid_login_origin/);
  const memory = storage();
  prepareLogin({ state, referrer: origin, storage: memory });
  await authorizationUrl({ state, referrer: origin, clientId: 'fixture', pageOrigin: 'https://login.example.test', storage: memory, cryptoImpl: webcrypto });
  assert.throws(() => consumeGoogleReturn({ fragment: '#id_token=a.b.c&state=wrong', storage: memory }), /login_failed/);
});
