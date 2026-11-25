import assert from 'node:assert/strict';
import { webcrypto } from 'node:crypto';
import { readFileSync } from 'node:fs';
import test from 'node:test';
import { verifyRelease } from './momso_release_manifest.mjs';
import { vmStartupScript } from './momso_anywhere_vm_startup.mjs';
import { resolveProductionTrust } from './momso_release_trust.mjs';

test('browser and VM use the shared root time vectors', async t => {
  const vectors = JSON.parse(readFileSync(new URL('./fixtures/root-time-vectors.json', import.meta.url), 'utf8'));
  const pairs = await Promise.all(Array.from({ length: 3 }, () => webcrypto.subtle.generateKey('Ed25519', true, ['sign', 'verify'])));
  const keys = await Promise.all(pairs.map(async pair => Buffer.from(await webcrypto.subtle.exportKey('spki', pair.publicKey)).toString('base64')));
  const encode = value => new TextEncoder().encode(JSON.stringify(value));
  const root = (version, expires, issuedAt) => encode({ version, ...(issuedAt ? { issuedAt } : {}), expires, threshold: 2, keys, releaseKey: keys[0] });
  const sign = async (raw, count) => {
    const result = {};
    for (let i = 0; i < count; i++) result[keys[i]] = Buffer.from(await webcrypto.subtle.sign('Ed25519', pairs[i].privateKey, raw)).toString('base64');
    return encode(result);
  };
  const response = raw => ({ ok: true, status: 200, arrayBuffer: async () => raw.buffer.slice(raw.byteOffset, raw.byteOffset + raw.byteLength) });
  for (const item of vectors.cases) await t.test(item.name, async () => {
    const bootstrap = root(0, vectors.bootstrapExpires), first = root(1, item.firstExpires, item.firstIssued), second = root(2, item.secondExpires, item.secondIssued);
    const files = { 'trust-root-1.json': first, 'trust-root-1.sigs.json': await sign(first, 2), 'trust-root-2.json': second, 'trust-root-2.sigs.json': await sign(second, item.signers) };
    const storage = { getItem: () => null, setItem: () => {} };
    const fetchImpl = async url => files[new URL(url).pathname.split('/').at(-1)] ? response(files[new URL(url).pathname.split('/').at(-1)]) : { ok: false, status: 404 };
    const config = { manifestUrl: 'https://fixture.test/release.json', publicKeySpki: keys[0], trustBootstrap: Buffer.from(bootstrap).toString('base64') };
    const promise = resolveProductionTrust(config, { fetchImpl, cryptoImpl: webcrypto, storage, now: Date.parse(vectors.now) });
    if (item.accepted) assert.equal(await promise, keys[0]); else await assert.rejects(promise);
  });
});

test('browser verifies production rotation, retains it, and passes the identical bootstrap to the VM', async () => {
  const pairs = await Promise.all(Array.from({ length: 4 }, () => webcrypto.subtle.generateKey('Ed25519', true, ['sign', 'verify'])));
  const keys = await Promise.all(pairs.map(async pair => Buffer.from(await webcrypto.subtle.exportKey('spki', pair.publicKey)).toString('base64')));
  const encode = value => new TextEncoder().encode(JSON.stringify(value));
  const bootstrap = encode({ version: 0, expires: '2099-01-01T00:00:00Z', threshold: 2, keys: keys.slice(0, 3), releaseKey: keys[0] });
  const next = encode({ version: 1, issuedAt: '2025-01-01T00:00:00Z', expires: '2099-01-01T00:00:00Z', threshold: 2, keys: keys.slice(1), releaseKey: keys[3] });
  const signatures = {};
  for (const index of [1, 2]) signatures[keys[index]] = Buffer.from(await webcrypto.subtle.sign('Ed25519', pairs[index].privateKey, next)).toString('base64');
  const manifest = encode({ sequence: 2, version: '0.1.1', channel: 'production', bundleUrl: 'https://fixture.test/bundle', bundleSha256: 'a'.repeat(64), gatewayImage: 'ghcr.io/example/gateway@sha256:' + 'b'.repeat(64) });
  const signature = new Uint8Array(await webcrypto.subtle.sign('Ed25519', pairs[3].privateKey, manifest));
  const config = { manifestUrl: 'https://fixture.test/release.json', publicKeySpki: keys[0], trustBootstrap: Buffer.from(bootstrap).toString('base64'), channel: 'production' };
  const state = new Map();
  const storage = { getItem: key => state.get(key) ?? null, setItem: (key, value) => state.set(key, value) };
  let rotation = false;
  const response = bytes => ({ ok: true, status: 200, arrayBuffer: async () => bytes.buffer.slice(bytes.byteOffset, bytes.byteOffset + bytes.byteLength) });
  const fetchImpl = async url => {
    if (url.endsWith('trust-root-1.json')) return rotation ? response(next) : { ok: false, status: 404 };
    if (url.endsWith('trust-root-1.sigs.json')) return response(encode(signatures));
    if (url.endsWith('trust-root-2.json')) return { ok: false, status: 404 };
    if (url.endsWith('release.json.sig')) return response(signature);
    if (url.endsWith('release.json')) return response(manifest);
    throw new Error('unexpected');
  };
  await assert.rejects(verifyRelease(config, { fetchImpl, cryptoImpl: webcrypto, storage }), { step: 'release_not_ready' });
  rotation = true;
  assert.equal((await verifyRelease(config, { fetchImpl, cryptoImpl: webcrypto, storage })).sequence, 2);
  rotation = false;
  assert.equal((await verifyRelease(config, { fetchImpl, cryptoImpl: webcrypto, storage })).sequence, 2);
  const script = vmStartupScript({ project: 'fixture-project', secret: 'momso-transport-aaaaaaaaaaaa', bundleUrl: 'https://fixture.test/bundle', bundleSha256: 'a'.repeat(64), update: config });
  assert(script.includes(config.trustBootstrap));
  assert(script.includes("config['trustState']='/var/lib/momso-update-trust'"));
  const duplicate = Buffer.from(Buffer.from(bootstrap).toString().replace('"version":0', '"version":0,"version":0')).toString('base64');
  await assert.rejects(verifyRelease({ ...config, trustBootstrap: duplicate }, { fetchImpl, cryptoImpl: webcrypto, storage }), { step: 'release_not_ready' });
});
