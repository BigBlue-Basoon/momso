// Public production root chain. Every saved rotation is reverified from bootstrap.
const LIMIT = 65536;
const PREFIX = '302a300506032b6570032100';
const decoder = new TextDecoder('utf-8', { fatal: true });

function decode(value) {
  if (typeof value !== 'string' || !/^[A-Za-z0-9+/]+={0,2}$/.test(value)) throw new Error('root_invalid');
  const raw = Uint8Array.from(atob(value), c => c.charCodeAt(0));
  if (btoa(String.fromCharCode(...raw)) !== value) throw new Error('root_invalid');
  return raw;
}
function encode(raw) { let result = ''; for (const byte of raw) result += String.fromCharCode(byte); return btoa(result); }
function strictJson(raw) {
  const text = decoder.decode(raw); let index = 0;
  const ws = () => { while (/[\x20\t\r\n]/.test(text[index] ?? '')) index++; };
  const string = () => {
    const start = index++;
    while (index < text.length) {
      if (text[index] === '\\') { index += 2; continue; }
      if (text[index++] === '"') return JSON.parse(text.slice(start, index));
    }
    throw new Error('root_invalid');
  };
  const value = () => {
    ws();
    if (text[index] === '"') { string(); return; }
    if (text[index] === '{') {
      index++; ws(); const seen = new Set();
      if (text[index] === '}') { index++; return; }
      while (true) {
        ws(); if (text[index] !== '"') throw new Error('root_invalid');
        const key = string(); if (seen.has(key)) throw new Error('root_invalid'); seen.add(key);
        ws(); if (text[index++] !== ':') throw new Error('root_invalid'); value(); ws();
        if (text[index] === '}') { index++; return; }
        if (text[index++] !== ',') throw new Error('root_invalid');
      }
    }
    if (text[index] === '[') {
      index++; ws(); if (text[index] === ']') { index++; return; }
      while (true) { value(); ws(); if (text[index] === ']') { index++; return; } if (text[index++] !== ',') throw new Error('root_invalid'); }
    }
    const token = /^(?:true|false|null|-?(?:0|[1-9]\d*)(?:\.\d+)?(?:[eE][+-]?\d+)?)/.exec(text.slice(index));
    if (!token) throw new Error('root_invalid'); index += token[0].length;
  };
  value(); ws(); if (index !== text.length) throw new Error('root_invalid');
  return JSON.parse(text);
}
function parse(raw) {
  if (raw.length > LIMIT) throw new Error('root_invalid');
  const value = strictJson(raw);
  const fields = value && Object.keys(value).sort().join(',');
  if (!value || Array.isArray(value) || !['expires,keys,releaseKey,threshold,version', 'expires,issuedAt,keys,releaseKey,threshold,version'].includes(fields) || !Number.isSafeInteger(value.version) || value.version < 0 || value.threshold !== 2 || !Array.isArray(value.keys) || value.keys.length !== 3 || new Set(value.keys).size !== 3 || !value.keys.includes(value.releaseKey) || (value.version > 0 && !Object.hasOwn(value, 'issuedAt'))) throw new Error('root_invalid');
  timestamp(value.expires);
  if (Object.hasOwn(value, 'issuedAt')) timestamp(value.issuedAt);
  for (const key of value.keys) {
    const bytes = decode(key);
    if (bytes.length !== 44 || [...bytes.slice(0, 12)].map(x => x.toString(16).padStart(2, '0')).join('') !== PREFIX) throw new Error('root_invalid');
  }
  return value;
}
function timestamp(value) {
  if (typeof value !== 'string' || value.startsWith('0000-') || !/^\d{4}-\d{2}-\d{2}T\d{2}:\d{2}:\d{2}Z$/.test(value)) throw new Error('root_invalid');
  const time = Date.parse(value);
  if (!Number.isFinite(time) || new Date(time).toISOString().replace('.000Z', 'Z') !== value) throw new Error('root_invalid');
  return time;
}
async function verifyRotation(current, raw, signatures, cryptoImpl, now) {
  const next = parse(raw);
  const issued = timestamp(next.issuedAt);
  if (next.version !== current.version + 1 || issued > now || (Object.hasOwn(current, 'issuedAt') && issued < timestamp(current.issuedAt)) || issued >= timestamp(current.expires) || issued >= timestamp(next.expires) || !signatures || Array.isArray(signatures) || typeof signatures !== 'object' || Object.keys(signatures).length > 6) throw new Error('root_invalid');
  const valid = new Set();
  for (const key of new Set([...current.keys, ...next.keys])) {
    if (!Object.hasOwn(signatures, key)) continue;
    const sig = decode(signatures[key]);
    if (sig.length !== 64) continue;
    const imported = await cryptoImpl.subtle.importKey('spki', decode(key), { name: 'Ed25519' }, false, ['verify']);
    if (await cryptoImpl.subtle.verify('Ed25519', imported, sig, raw)) valid.add(key);
  }
  if (current.keys.filter(key => valid.has(key)).length < 2 || next.keys.filter(key => valid.has(key)).length < 2) throw new Error('root_invalid');
  return next;
}
async function fetchAsset(url, fetchImpl, optional = false) {
  const response = await fetchImpl(url, { cache: 'no-store', redirect: 'error', credentials: 'omit' });
  if (optional && response.status === 404) return null;
  if (!response.ok) throw new Error('root_unavailable');
  const raw = new Uint8Array(await response.arrayBuffer());
  if (raw.length > LIMIT) throw new Error('root_invalid');
  return raw;
}
export async function resolveProductionTrust(config, { fetchImpl = fetch, cryptoImpl = crypto, storage = globalThis.localStorage, now = Date.now() } = {}) {
  const bootstrap = decode(config.trustBootstrap);
  let root = parse(bootstrap);
  if (root.version !== 0 || root.releaseKey !== config.publicKeySpki) throw new Error('root_invalid');
  const key = 'momso.production.roots.v1:' + config.trustBootstrap;
  let chain = JSON.parse(storage.getItem(key) || '[]');
  if (!Array.isArray(chain) || chain.length > 128) throw new Error('root_invalid');
  for (const item of chain) root = await verifyRotation(root, decode(item.root), item.signatures, cryptoImpl, now);
  const manifest = new URL(config.manifestUrl);
  for (let count = chain.length; count < 128; count++) {
    const version = root.version + 1;
    const nextUrl = new URL(`trust-root-${version}.json`, manifest);
    const raw = await fetchAsset(nextUrl.href, fetchImpl, true);
    if (raw === null) break;
    const sigUrl = new URL(`trust-root-${version}.sigs.json`, manifest);
    const signatures = strictJson(await fetchAsset(sigUrl.href, fetchImpl));
    const next = await verifyRotation(root, raw, signatures, cryptoImpl, now);
    if (next.version !== version) throw new Error('root_invalid');
    chain.push({ root: encode(raw), signatures });
    storage.setItem(key, JSON.stringify(chain));
    root = next;
    if (count === 127) throw new Error('root_chain_too_long');
  }
  if ((Object.hasOwn(root, 'issuedAt') && timestamp(root.issuedAt) > now)) throw new Error('root_invalid');
  if (timestamp(root.expires) <= now) throw new Error('root_expired');
  return root.releaseKey;
}
