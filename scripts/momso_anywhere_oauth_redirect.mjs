import { SetupError } from './momso_anywhere_setup.mjs';

const KEY = 'momso.start.oauth.pending.v1';
const SCOPES = ['openid', 'email', 'https://www.googleapis.com/auth/cloud-platform'];
const ALLOWED_ACTIONS = new Set(['start', 'find', 'projects', 'billing', 'destroy', 'deletion_preview']);

function encode(bytes) {
  return btoa(String.fromCharCode(...bytes)).replaceAll('+', '-').replaceAll('/', '_').replace(/=+$/, '');
}

export function beginRedirect({ clientId, action, projectId = null, billingAccountName = null, installationId = null, discardData = false, storage, location, cryptoImpl = crypto, now = Date.now() }) {
  if (!/^[A-Za-z0-9._-]+\.apps\.googleusercontent\.com$/.test(clientId) || !ALLOWED_ACTIONS.has(action)) throw new SetupError('invalid_authorization');
  if (projectId !== null && !/^[a-z][a-z0-9-]{4,28}[a-z0-9]$/.test(projectId)) throw new SetupError('invalid_project');
  if (billingAccountName !== null && !/^billingAccounts\/[A-Za-z0-9-]+$/.test(billingAccountName)) throw new SetupError('invalid_billing_account');
  if (installationId !== null && !/^[a-f0-9]{24}$/.test(installationId)) throw new SetupError('invalid_checkpoint');
  if (typeof discardData !== 'boolean' || (discardData && action !== 'destroy')) throw new SetupError('invalid_authorization');
  const page = new URL(location.href);
  if (page.protocol !== 'https:' || page.pathname !== '/start/' || page.search || page.hash) throw new SetupError('invalid_redirect');
  const state = encode(cryptoImpl.getRandomValues(new Uint8Array(32)));
  try { storage.setItem(KEY, JSON.stringify({ state, action, projectId, billingAccountName, installationId, discardData, created: now, origin: page.origin, clientId })); }
  catch { throw new SetupError('checkpoint_unavailable'); }
  const target = new URL('https://accounts.google.com/o/oauth2/v2/auth');
  target.search = new URLSearchParams({ client_id: clientId, redirect_uri: page.origin + '/start/', response_type: 'token', scope: SCOPES.join(' '), state }).toString();
  location.assign(target.href);
  return { redirecting: true };
}

export function consumeRedirect({ fragment, storage, origin, clientId, now = Date.now() }) {
  let saved;
  try { saved = JSON.parse(storage.getItem(KEY) || 'null'); storage.removeItem(KEY); }
  catch { throw new SetupError('checkpoint_unavailable'); }
  const values = new URLSearchParams((fragment || '').replace(/^#/, ''));
  if (!saved || saved.origin !== origin || saved.clientId !== clientId || !ALLOWED_ACTIONS.has(saved.action) || !Number.isFinite(saved.created) || now < saved.created || now - saved.created > 15 * 60 * 1000 || values.getAll('state').length !== 1 || values.get('state') !== saved.state) throw new SetupError('authorization_invalid');
  if (values.has('error')) throw new SetupError('authorization_declined');
  const token = values.get('access_token');
  const expires = Number(values.get('expires_in'));
  const granted = new Set((values.get('scope') || '').split(/\s+/));
  if (values.getAll('access_token').length !== 1 || !token || token.length > 16384 || /[\s\x00-\x1f]/.test(token) || values.get('token_type')?.toLowerCase() !== 'bearer' || !Number.isFinite(expires) || expires <= 0 || !granted.has('openid') || !(granted.has('email') || granted.has('https://www.googleapis.com/auth/userinfo.email')) || !granted.has(SCOPES[2])) throw new SetupError('authorization_invalid');
  return { ...saved, token, expiresAt: now + expires * 1000 };
}
