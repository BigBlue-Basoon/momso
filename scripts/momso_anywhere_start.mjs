import { providerFromToken } from './momso_anywhere_google_browser.mjs';
import { startAnywhere, SetupError } from './momso_anywhere_setup.mjs';
import { beginRedirect, consumeRedirect } from './momso_anywhere_oauth_redirect.mjs';
import { hasOAuthFragment, takeOAuthFragment } from './momso_anywhere_oauth_capture.mjs';

const RETRY_KEY = 'momso.start.retry.v1';
const CLOUD_RETURN_KEY = 'momso.start.cloud-return.v1';
const CLOUD_SETUP_URLS = Object.freeze({
  cloud: 'https://console.cloud.google.com/',
  billing: 'https://console.cloud.google.com/billing',
  trial: 'https://cloud.google.com/free',
  quota: 'https://console.cloud.google.com/projectcreate',
});
let authorization = null;
export function hasPendingRedirect() { return hasOAuthFragment(); }
export function clearStartAuthorization() { authorization = null; }

// Only fixed Google URLs are accepted; the saved retry contains no credentials.
export function openGoogleSetup({ destination, session = sessionStorage, location = window.location }) {
  const target = CLOUD_SETUP_URLS[destination];
  if (!target || !Object.hasOwn(CLOUD_SETUP_URLS, destination)) throw new SetupError('invalid_destination');
  try {
    if (!session.getItem(RETRY_KEY)) throw new SetupError('checkpoint_unavailable');
    session.setItem(CLOUD_RETURN_KEY, '1');
  } catch (error) {
    if (error instanceof SetupError) throw error;
    throw new SetupError('checkpoint_unavailable');
  }
  location.assign(target);
}

// User gesture -> Google authorization -> search only this account's owned installations.
export async function findFromGesture({ google, clientId, release, authorize, session = globalThis.sessionStorage, location = globalThis.window?.location, cryptoImpl = crypto, storage = globalThis.localStorage, fetchImpl = fetch, providerFactory = providerFromToken }) {
  if (!authorize) return runGesture({ action: 'find', clientId, release, session, location, cryptoImpl, storage, fetchImpl, providerFactory });
  const { provider } = await authorize({ google, clientId, release });
  return provider.findExisting();
}

// Called directly from the screen's Start button; neither Google token nor identity is stored.
export async function startFromGesture({ google, clientId, release, billingAccountName = null, projectId = null, onProgress, storage = localStorage, session = globalThis.sessionStorage, location = globalThis.window?.location, cryptoImpl = crypto, authorize, fetchImpl = fetch, providerFactory = providerFromToken }) {
  if (!authorize) return runGesture({ action: 'start', clientId, release, projectId, billingAccountName, onProgress, storage, session, location, cryptoImpl, fetchImpl, providerFactory });
  const { provider, ownerSub } = await authorize({ google, clientId, release, billingAccountName, projectId });
  return startWithProvider({ provider, ownerSub, clientId, projectId, onProgress, storage, cryptoImpl });
}

async function startWithProvider({ provider, ownerSub, clientId, projectId, onProgress, storage, cryptoImpl }) {
  await provider.preflight();
  const digest = await cryptoImpl.subtle.digest('SHA-256', new TextEncoder().encode(`${clientId}:${ownerSub}${projectId ? ':' + projectId : ''}`));
  const key = `momso-setup-${[...new Uint8Array(digest)].map(byte => byte.toString(16).padStart(2, '0')).join('')}`;
  let installationId;
  try {
    installationId = storage.getItem(key);
    if (!installationId) {
      installationId = [...cryptoImpl.getRandomValues(new Uint8Array(12))].map(byte => byte.toString(16).padStart(2, '0')).join('');
      storage.setItem(key, installationId);
    }
  } catch { throw new SetupError('checkpoint_unavailable'); }
  if (!/^[a-f0-9]{24}$/.test(installationId)) throw new SetupError('invalid_checkpoint');
  return startAnywhere(provider, { installationId }, onProgress);
}

export async function billingAccountsFromGesture({ google, clientId, release, projectId = null, billingAccountName = null, authorize, session = globalThis.sessionStorage, location = globalThis.window?.location, cryptoImpl = crypto, storage = globalThis.localStorage, fetchImpl = fetch, providerFactory = providerFromToken }) {
  if (!authorize) return runGesture({ action: 'billing', clientId, release, projectId, billingAccountName, session, location, cryptoImpl, storage, fetchImpl, providerFactory });
  const { provider } = await authorize({ google, clientId, release });
  return provider.billingAccounts();
}

export async function projectsFromGesture({ google, clientId, release, projectId = null, billingAccountName = null, authorize, session = globalThis.sessionStorage, location = globalThis.window?.location, cryptoImpl = crypto, storage = globalThis.localStorage, fetchImpl = fetch, providerFactory = providerFromToken }) {
  if (!authorize) return runGesture({ action: 'projects', clientId, release, projectId, billingAccountName, session, location, cryptoImpl, storage, fetchImpl, providerFactory });
  const { provider } = await authorize({ google, clientId, release });
  return provider.projects();
}

export async function destroyFromGesture({ clientId, release, projectId, installationId, onProgress, session = globalThis.sessionStorage, location = globalThis.window?.location, cryptoImpl = crypto, storage = globalThis.localStorage, fetchImpl = fetch, providerFactory = providerFromToken }) {
  if (!projectId || !installationId) throw new SetupError('invalid_checkpoint');
  return runGesture({ action: 'destroy', clientId, release, projectId, installationId, onProgress, session, location, cryptoImpl, storage, fetchImpl, providerFactory });
}

async function performRequest({ request, clientId, release, onProgress, storage, session, cryptoImpl, fetchImpl, providerFactory }) {
  try { session.setItem(RETRY_KEY, JSON.stringify({ action: request.action, projectId: request.projectId, billingAccountName: request.billingAccountName, installationId: request.installationId })); }
  catch { throw new SetupError('checkpoint_unavailable'); }
  const { provider, ownerSub } = await providerFactory({ ...request, clientId, release, fetchImpl });
  let value;
  if (request.action === 'find') value = await provider.findExisting();
  else if (request.action === 'projects') value = await provider.projects();
  else if (request.action === 'billing') value = await provider.billingAccounts();
  else if (request.action === 'start') value = await startWithProvider({ provider, ownerSub, clientId, projectId: request.projectId, onProgress, storage, cryptoImpl });
  else if (request.action === 'destroy') {
    if (!request.projectId || !request.installationId) throw new SetupError('invalid_checkpoint');
    const key = `momso.destroy.${request.projectId}.${request.installationId}`;
    storage.setItem(key, JSON.stringify({ projectId: request.projectId, installationId: request.installationId, remaining: ['unknown'] }));
    value = await provider.destroy(request.installationId, onProgress);
    if (value.remaining.length) storage.setItem(key, JSON.stringify({ projectId: request.projectId, installationId: request.installationId, remaining: value.remaining }));
    else storage.removeItem(key);
  }
  else throw new SetupError('invalid_authorization');
  session.removeItem(RETRY_KEY);
  return value;
}

async function runGesture(options) {
  const { action, clientId, session, location, cryptoImpl } = options;
  const request = { action, projectId: options.projectId ?? null, billingAccountName: options.billingAccountName ?? null, installationId: options.installationId ?? null };
  const redirect = () => beginRedirect({ ...request, clientId, storage: session, location, cryptoImpl });
  if (!authorization || authorization.clientId !== clientId || authorization.origin !== location.origin || Date.now() >= authorization.expiresAt) {
    clearStartAuthorization();
    return redirect();
  }
  try {
    return await performRequest({ ...options, request: { ...request, token: authorization.token, expiresAt: authorization.expiresAt } });
  } catch (error) {
    if (error?.step !== 'authorization_expired') throw error;
    clearStartAuthorization();
    return redirect();
  }
}

export async function resumeFromRedirect({ clientId, release, onProgress, storage = localStorage, session = sessionStorage, location = window.location, cryptoImpl = crypto, fetchImpl = fetch, fragment = takeOAuthFragment(), providerFactory = providerFromToken, includeSelection = false }) {
  clearStartAuthorization();
  const request = consumeRedirect({ fragment, storage: session, origin: location.origin, clientId });
  authorization = { token: request.token, expiresAt: request.expiresAt, clientId, origin: location.origin };
  try {
    const value = await performRequest({ request, clientId, release, onProgress, storage, session, cryptoImpl, fetchImpl, providerFactory });
    return { action: request.action, value, ...(includeSelection ? { selection: { projectId: request.projectId, billingAccountName: request.billingAccountName } } : {}) };
  } catch (error) {
    if (error?.step === 'authorization_expired') clearStartAuthorization();
    throw error;
  }
}

// The screen calls this once after loading, including on ordinary visits.
export async function resumeAfterRedirect(options) {
  const fragment = options.fragment ?? takeOAuthFragment();
  if (!fragment) {
    const session = options.session ?? sessionStorage;
    try {
      if (session.getItem(CLOUD_RETURN_KEY) !== '1') return null;
      session.removeItem(CLOUD_RETURN_KEY);
    } catch { throw new SetupError('checkpoint_unavailable'); }
    return retryLastAction(options);
  }
  try {
    const { action, value, selection } = await resumeFromRedirect({ ...options, fragment, includeSelection: true });
    return screenResult(action, value, selection);
  } catch (error) {
    if (error?.step !== 'authorization_expired') throw error;
    retryFromGesture(options);
    return { kind: 'redirecting' };
  }
}

function screenResult(action, value, selection) {
  if (action === 'projects') return { kind: action, projects: value, selection };
  if (action === 'billing') return { kind: action, billingAccounts: value, selection };
  return { kind: action, result: value, selection };
}

// Unlike the legacy synchronous retry, this returns the completed operation to the screen.
export async function retryLastAction({ clientId, release, onProgress, storage = localStorage, session = sessionStorage, location = window.location, cryptoImpl = crypto, fetchImpl = fetch, providerFactory = providerFromToken }) {
  let request;
  try { request = JSON.parse(session.getItem(RETRY_KEY) || 'null'); }
  catch { throw new SetupError('checkpoint_unavailable'); }
  if (!request) throw new SetupError('checkpoint_unavailable');
  const value = await runGesture({ ...request, clientId, release, onProgress, storage, session, location, cryptoImpl, fetchImpl, providerFactory });
  if (value?.redirecting === true) return { kind: 'redirecting' };
  return screenResult(request.action, value, { projectId: request.projectId, billingAccountName: request.billingAccountName });
}

export function retryFromGesture({ clientId, session = sessionStorage, location = window.location, cryptoImpl = crypto }) {
  let request;
  try { request = JSON.parse(session.getItem(RETRY_KEY) || 'null'); }
  catch { throw new SetupError('checkpoint_unavailable'); }
  if (!request) throw new SetupError('checkpoint_unavailable');
  return beginRedirect({ ...request, clientId, storage: session, location, cryptoImpl });
}
