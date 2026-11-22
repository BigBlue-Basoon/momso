// Browser implementation of the S38 Ed25519 raw-manifest verification contract.
import { SetupError } from './momso_anywhere_setup.mjs';

function bytes64(value) {
  if (typeof value !== 'string' || !/^[A-Za-z0-9+/]+={0,2}$/.test(value)) throw new SetupError('release_not_ready');
  try { return Uint8Array.from(atob(value), c => c.charCodeAt(0)); } catch { throw new SetupError('release_not_ready'); }
}

async function download(url, fetchImpl, limit) {
  let parsed;
  try { parsed = new URL(url); } catch { throw new SetupError('release_not_ready'); }
  if (parsed.protocol !== 'https:' || parsed.username || parsed.password || parsed.hash) throw new SetupError('release_not_ready');
  let response;
  try { response = await fetchImpl(parsed.href, { cache: 'no-store', redirect: 'error', credentials: 'omit' }); }
  catch { throw new SetupError('release_not_ready'); }
  if (!response.ok) throw new SetupError('release_not_ready');
  const data = new Uint8Array(await response.arrayBuffer());
  if (data.length > limit) throw new SetupError('release_not_ready');
  return data;
}

export async function verifyRelease(config, { fetchImpl = fetch, cryptoImpl = crypto } = {}) {
  try {
    const url = new URL(config.manifestUrl);
    const signature = new URL(url.href);
    signature.pathname += '.sig';
    const [manifest, sig] = await Promise.all([download(url.href, fetchImpl, 65536), download(signature.href, fetchImpl, 1024)]);
    const spki = bytes64(config.publicKeySpki);
    const prefix = '302a300506032b6570032100';
    if (sig.length !== 64 || spki.length !== 44 || [...spki.slice(0, 12)].map(v => v.toString(16).padStart(2, '0')).join('') !== prefix) throw new Error();
    const key = await cryptoImpl.subtle.importKey('spki', spki, { name: 'Ed25519' }, false, ['verify']);
    if (!await cryptoImpl.subtle.verify('Ed25519', key, sig, manifest)) throw new Error();
    const data = JSON.parse(new TextDecoder('utf-8', { fatal: true }).decode(manifest));
    const bundle = new URL(data.bundleUrl);
    const channel = config.channel ?? 'production';
    if (!['production', 'development'].includes(channel) || (data.channel ?? 'production') !== channel || !Number.isSafeInteger(data.sequence) || data.sequence < 1 || !/^[0-9]+\.[0-9]+\.[0-9]+$/.test(data.version) || !/^[a-f0-9]{64}$/.test(data.bundleSha256) || !/^[a-zA-Z0-9._/-]+@sha256:[a-f0-9]{64}$/.test(data.gatewayImage) || bundle.protocol !== 'https:' || bundle.username || bundle.password || bundle.hash) throw new Error();
    return { ...data, loginOrigin: config.loginOrigin, trustedGatewayImages: config.trustedGatewayImages ?? [] };
  } catch { throw new SetupError('release_not_ready'); }
}
