// The login page and gateway exchange identity only in fragments and same-origin POSTs.
const STORAGE_KEY = 'momso.oidc.pending.v1';
const AUTHORIZE = 'https://accounts.google.com/o/oauth2/v2/auth';

function base64url(bytes) {
  return btoa(String.fromCharCode(...bytes)).replaceAll('+', '-').replaceAll('/', '_').replace(/=+$/, '');
}

function fromBase64Url(value) {
  const normalized = value.replaceAll('-', '+').replaceAll('_', '/');
  return JSON.parse(atob(normalized + '='.repeat((4 - normalized.length % 4) % 4)));
}

function allowedGateway(origin) {
  try {
    const url = new URL(origin);
    return url.protocol === 'https:' && url.origin === origin && url.hostname.endsWith('.run.app');
  } catch { return false; }
}

export function validateRedirectContext(state, referrer) {
  try {
    if (typeof state !== 'string' || state.length > 8192 || !/^[A-Za-z0-9_-]+\.[A-Za-z0-9_-]+$/.test(state)) throw new Error();
    const payload = fromBase64Url(state.split('.')[0]);
    if (!allowedGateway(payload.origin) || (referrer && new URL(referrer).origin !== payload.origin)) throw new Error();
    return { state, origin: payload.origin };
  } catch { throw new Error('invalid_login_origin'); }
}

export function prepareLogin({ state, referrer, storage }) {
  const context = validateRedirectContext(state, referrer);
  storage.setItem(STORAGE_KEY, JSON.stringify(context));
  return context;
}

export function savedLoginContext(storage) {
  try {
    const saved = JSON.parse(storage.getItem(STORAGE_KEY) || 'null');
    return validateRedirectContext(saved.state, saved.origin);
  } catch { return null; }
}

export function needsFreshChallenge(state, now = Date.now()) {
  try {
    const issued = fromBase64Url(state.split('.')[0]).iat;
    const age = Math.floor(now / 1000) - issued;
    return !Number.isInteger(issued) || age < 0 || age >= 240;
  } catch { return true; }
}

export async function authorizationUrl({ state, referrer, clientId, pageOrigin, storage, cryptoImpl = crypto }) {
  const current = validateRedirectContext(state, referrer);
  const saved = JSON.parse(storage.getItem(STORAGE_KEY) || 'null');
  const site = new URL(pageOrigin);
  if (site.protocol !== 'https:' || site.origin !== pageOrigin || !saved || saved.state !== current.state || saved.origin !== current.origin || !clientId) throw new Error('invalid_login_context');
  const correlation = base64url(cryptoImpl.getRandomValues(new Uint8Array(32)));
  const bytes = await cryptoImpl.subtle.digest('SHA-256', new TextEncoder().encode(state));
  const nonce = base64url(new Uint8Array(bytes));
  storage.setItem(STORAGE_KEY, JSON.stringify({ ...current, correlation }));
  const url = new URL(AUTHORIZE);
  url.search = new URLSearchParams({ client_id: clientId, redirect_uri: pageOrigin + '/', response_type: 'id_token', response_mode: 'fragment', scope: 'openid email', nonce, state: correlation }).toString();
  return url.href;
}

export function consumeGoogleReturn({ fragment, storage }) {
  const data = new URLSearchParams(fragment.replace(/^#/, ''));
  const saved = JSON.parse(storage.getItem(STORAGE_KEY) || 'null');
  const context = savedLoginContext(storage);
  if (context) storage.setItem(STORAGE_KEY, JSON.stringify(context));
  else storage.removeItem(STORAGE_KEY);
  if (!context || !saved.correlation || data.getAll('state').length !== 1 || data.get('state') !== saved.correlation || data.has('error')) throw new Error('login_failed');
  const values = data.getAll('id_token');
  if (values.length !== 1 || values[0].length > 16384 || !/^[A-Za-z0-9_-]+\.[A-Za-z0-9_-]+\.[A-Za-z0-9_-]+$/.test(values[0])) throw new Error('login_failed');
  storage.removeItem(STORAGE_KEY);
  const destination = new URL('/__momso/auth/return', saved.origin);
  destination.hash = new URLSearchParams({ state: saved.state, id_token: values[0] }).toString();
  return destination.href;
}
