import { webcrypto } from 'node:crypto';

// Fresh test key only; the private key never leaves this fixture.
export async function signedFixture(overrides = {}) {
  const keys = await webcrypto.subtle.generateKey('Ed25519', true, ['sign', 'verify']);
  const spki = new Uint8Array(await webcrypto.subtle.exportKey('spki', keys.publicKey));
  const manifest = { sequence: 1, version: '0.1.1', bundleUrl: 'https://example.test/release.tar.gz', bundleSha256: 'f'.repeat(64), gatewayImage: `registry.example.test/team/gateway@sha256:${'c'.repeat(64)}`, ...overrides };
  const raw = new TextEncoder().encode(JSON.stringify(manifest));
  const signature = new Uint8Array(await webcrypto.subtle.sign('Ed25519', keys.privateKey, raw));
  const release = { manifestUrl: 'https://fixture.test/release.json', publicKeySpki: Buffer.from(spki).toString('base64'), loginOrigin: 'https://login.example.test' };
  const response = data => ({ ok: true, arrayBuffer: async () => data.buffer.slice(data.byteOffset, data.byteOffset + data.byteLength) });
  return { manifest, release, raw, signature, response };
}
