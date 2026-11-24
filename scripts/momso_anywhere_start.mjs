import { providerFromToken } from './momso_anywhere_google_browser.mjs';
import { startAnywhere, SetupError } from './momso_anywhere_setup.mjs';
import { beginRedirect, consumeRedirect } from './momso_anywhere_oauth_redirect.mjs';
import { hasOAuthFragment, takeOAuthFragment } from './momso_anywhere_oauth_capture.mjs';
import { parsePauseFragment, savePauseForRedirect, takePauseAfterRedirect, clearPause } from './momso_anywhere_pause_transfer.mjs';
import { deletionResult } from './momso_anywhere_delete_result.mjs';

const RETRY_KEY = 'momso.start.retry.v1';
const CLOUD_RETURN_KEY = 'momso.start.cloud-return.v1';
const CLOUD_SETUP_URLS = Object.freeze({
  cloud: 'https://console.cloud.google.com/',
  billing: 'https://console.cloud.google.com/billing',
  trial: 'https://cloud.google.com/free',
  quota: 'https://console.cloud.google.com/projectcreate',
});
let authorization = null;
let pendingPause = null;
let activePause = null;
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

export async function destroyFromGesture({ clientId, release, projectId, installationId, discardData = false, onProgress, session = globalThis.sessionStorage, location = globalThis.window?.location, cryptoImpl = crypto, storage = globalThis.localStorage, fetchImpl = fetch, providerFactory = providerFromToken }) {
  try {
    if (!projectId || !installationId) throw new SetupError('invalid_checkpoint');
    if (typeof discardData !== 'boolean') throw new SetupError('invalid_authorization');
    if (!discardData && (!pendingPause || pendingPause.projectId !== projectId || pendingPause.installationId !== installationId)) throw new SetupError('pause_requires_app_session');
    return await runGesture({ action: 'destroy', clientId, release, projectId, installationId, discardData, onProgress, session, location, cryptoImpl, storage, fetchImpl, providerFactory });
  } catch (error) { return failedDeletion(error, discardData, { storage, projectId, installationId }); }
}

function failedDeletion(error, discardData, { storage, projectId, installationId } = {}) {
  const reason = /^[a-z][a-z0-9_]{0,79}$/.test(error?.step ?? '') ? error.step : 'delete_failed';
  let previous = {};
  try {
    const saved = JSON.parse(storage?.getItem(`momso.destroy.${projectId}.${installationId}`) ?? 'null');
    if (saved?.projectId === projectId && saved?.installationId === installationId && saved.deletion && typeof saved.deletion === 'object') previous = saved.deletion;
  } catch { /* Prior billing evidence is unavailable. */ }
  return deletionResult({ ...previous, mode: discardData ? 'delete_all' : 'pause', failed: true, reason });
}

export async function cancelPauseFromGesture({ session = globalThis.sessionStorage, fetchImpl = fetch } = {}) {
  if (activePause) {
    if (activePause.vmState !== 'not_started') throw new SetupError('vm_deletion_unconfirmed');
    activePause.cancelled = true;
    return { ok: true, pending: true };
  }
  const pause = pendingPause ?? takePauseAfterRedirect(session);
  pendingPause = null;
  clearPause(session);
  if (!pause) throw new SetupError('pause_requires_app_session');
  let response, result;
  try {
    response = await fetchImpl(pause.gateway + '/__momso/pause/release', { method: 'POST', mode: 'cors', credentials: 'omit', headers: { 'Content-Type': 'application/json' }, body: JSON.stringify({ ticket: pause.ticket }), cache: 'no-store', redirect: 'error' });
    result = await response.json();
  } catch { throw new SetupError('maintenance_release_unconfirmed'); }
  if (!response.ok || !result?.ok) throw new SetupError('maintenance_release_unconfirmed');
  return result;
}

export async function deletionPreviewFromGesture({ clientId, release, projectId, installationId, session = globalThis.sessionStorage, location = globalThis.window?.location, cryptoImpl = crypto, storage = globalThis.localStorage, fetchImpl = fetch, providerFactory = providerFromToken }) {
  if (!projectId || !installationId) throw new SetupError('invalid_checkpoint');
  return runGesture({ action: 'deletion_preview', clientId, release, projectId, installationId, session, location, cryptoImpl, storage, fetchImpl, providerFactory });
}

async function performRequest({ request, clientId, release, onProgress, storage, session, cryptoImpl, fetchImpl, providerFactory }) {
  if (request.action === 'destroy' && request.discardData !== true && !pendingPause) throw new SetupError('pause_requires_app_session');
  try { session.setItem(RETRY_KEY, JSON.stringify({ action: request.action, projectId: request.projectId, billingAccountName: request.billingAccountName, installationId: request.installationId, discardData: request.discardData === true })); }
  catch { throw new SetupError('checkpoint_unavailable'); }
  const { provider, ownerSub } = await providerFactory({ ...request, clientId, release, fetchImpl });
  let value;
  if (request.action === 'find') value = await provider.findExisting();
  else if (request.action === 'projects') value = await provider.projects();
  else if (request.action === 'billing') value = await provider.billingAccounts();
  else if (request.action === 'deletion_preview') value = await provider.deletionPreview(request.installationId);
  else if (request.action === 'start') value = await startWithProvider({ provider, ownerSub, clientId, projectId: request.projectId, onProgress, storage, cryptoImpl });
  else if (request.action === 'destroy') {
    if (!request.projectId || !request.installationId) throw new SetupError('invalid_checkpoint');
    if (!request.discardData) {
      const pause = pendingPause;
      if (!pause || pause.projectId !== request.projectId || pause.installationId !== request.installationId || pause.expiresAt <= Date.now()) throw new SetupError('invalid_pause_ticket');
      if (await provider.pauseGateway(request.installationId) !== pause.gateway) throw new SetupError('resource_ownership');
      pendingPause = null;
      clearPause(session);
      const call = async (action, ticket) => {
        let response;
        try { response = await fetchImpl(pause.gateway + '/__momso/pause/' + action, { method: 'POST', mode: 'cors', credentials: 'omit', headers: { 'Content-Type': 'application/json' }, body: JSON.stringify({ ticket }), cache: 'no-store', redirect: 'error' }); }
        catch { throw new SetupError('vm_unreachable'); }
        let result;
        try { result = await response.json(); } catch { throw new SetupError('vm_unreachable'); }
        if (!response.ok || !result?.ok) throw new SetupError(['invalid_pause_ticket', 'maintenance_release_unconfirmed', 'backup_not_connected', 'backup_freshness_unverified', 'vm_unreachable'].includes(result?.reason ?? result?.error) ? (result.reason ?? result.error) : 'vm_unreachable');
        return result;
      };
      let vmState = 'not_started';
      const active = { vmState, cancelled: false };
      activePause = active;
      let confirmed;
      try { confirmed = await call('confirm', pause.ticket); }
      catch (error) { if (activePause === active) activePause = null; throw error; }
      if (!/^[A-Za-z0-9_-]{32,128}$/.test(confirmed.releaseTicket ?? '')) {
        if (activePause === active) activePause = null;
        throw new SetupError('maintenance_release_unconfirmed');
      }
      try {
        if (active.cancelled) throw new SetupError('pause_cancelled');
        const removed = await provider.destroy(request.installationId, onProgress, { onVmState: state => {
          if (state === 'unknown' && active.cancelled) throw new SetupError('pause_cancelled');
          vmState = state; active.vmState = state;
        } });
        if (vmState !== 'deleted') throw new SetupError('vm_deletion_unconfirmed');
        value = deletionResult({ ...removed, status: removed.remaining.length ? 'pending' : 'complete' });
      } catch (error) {
        if (vmState === 'not_started' || vmState === 'failed') {
          try { await call('release', confirmed.releaseTicket); }
          catch { throw new SetupError('maintenance_release_unconfirmed'); }
        }
        if (error?.step === 'authorization_expired') throw new SetupError(vmState === 'not_started' || vmState === 'failed' ? 'pause_requires_app_session' : 'vm_deletion_unconfirmed');
        throw error;
      } finally {
        if (activePause === active) activePause = null;
      }
      session.removeItem(RETRY_KEY);
      return value;
    }
    const key = `momso.destroy.${request.projectId}.${request.installationId}`;
    let previous = null;
    try {
      const saved = JSON.parse(storage.getItem(key) ?? 'null');
      if (saved?.projectId === request.projectId && saved?.installationId === request.installationId) previous = saved.deletion ?? null;
    } catch { throw new SetupError('invalid_checkpoint'); }
    storage.setItem(key, JSON.stringify({ projectId: request.projectId, installationId: request.installationId, remaining: ['unknown'], deletion: previous }));
    const infrastructure = await provider.destroy(request.installationId, onProgress);
    const deletion = await provider.discardBackup(request.installationId, previous);
    Object.assign(deletion.resources ??= {}, infrastructure.resources ?? {});
    for (const name of ['vm', 'disk', 'gateway', 'network', 'secrets', 'iam', 'iam_bindings']) deletion.resources[name] ??= { status: 'unverified', reason: 'not_inventoried' };
    const done = ['bucket', 'kms', 'secret_vault'].every(name => deletion.resources?.[name]?.status === 'deleted');
    value = deletionResult({ ...deletion, remaining: infrastructure.remaining, status: done && !infrastructure.remaining.length ? 'complete' : 'pending' });
    if (value.status === 'pending') storage.setItem(key, JSON.stringify({ projectId: request.projectId, installationId: request.installationId, remaining: value.remaining, deletion }));
    else storage.removeItem(key);
  }
  else throw new SetupError('invalid_authorization');
  session.removeItem(RETRY_KEY);
  return value;
}

async function runGesture(options) {
  const { action, clientId, session, location, cryptoImpl } = options;
  const request = { action, projectId: options.projectId ?? null, billingAccountName: options.billingAccountName ?? null, installationId: options.installationId ?? null, discardData: options.discardData === true };
  const redirect = async () => {
    try { return beginRedirect({ ...request, clientId, storage: session, location, cryptoImpl }); }
    catch (error) {
      if (action === 'destroy' && !request.discardData) {
        try { await cancelPauseFromGesture({ session, fetchImpl: options.fetchImpl }); }
        catch (releaseError) { if (releaseError?.step !== 'pause_requires_app_session') throw releaseError; }
      }
      throw error;
    }
  };
  if (!authorization || authorization.clientId !== clientId || authorization.origin !== location.origin || Date.now() >= authorization.expiresAt) {
    clearStartAuthorization();
    if (action === 'destroy' && !request.discardData) { savePauseForRedirect(pendingPause, session); pendingPause = null; }
    return redirect();
  }
  try {
    return await performRequest({ ...options, request: { ...request, token: authorization.token, expiresAt: authorization.expiresAt } });
  } catch (error) {
    if (error?.step !== 'authorization_expired') throw error;
    clearStartAuthorization();
    if (action === 'destroy' && !request.discardData) { if (!pendingPause) throw new SetupError('pause_requires_app_session'); savePauseForRedirect(pendingPause, session); pendingPause = null; }
    return redirect();
  }
}

export async function resumeFromRedirect({ clientId, release, onProgress, storage = localStorage, session = sessionStorage, location = window.location, cryptoImpl = crypto, fetchImpl = fetch, fragment = takeOAuthFragment(), providerFactory = providerFromToken, includeSelection = false }) {
  clearStartAuthorization();
  let request;
  try { request = consumeRedirect({ fragment, storage: session, origin: location.origin, clientId }); }
  catch (error) {
    try { await cancelPauseFromGesture({ session, fetchImpl }); }
    catch (releaseError) { if (releaseError?.step !== 'pause_requires_app_session') throw releaseError; }
    throw error;
  }
  if (request.action === 'destroy' && !request.discardData) pendingPause = takePauseAfterRedirect(session);
  authorization = { token: request.token, expiresAt: request.expiresAt, clientId, origin: location.origin };
  try {
    const value = await performRequest({ request, clientId, release, onProgress, storage, session, cryptoImpl, fetchImpl, providerFactory });
    return { action: request.action, value, ...(includeSelection ? { selection: { projectId: request.projectId, billingAccountName: request.billingAccountName } } : {}) };
  } catch (error) {
    if (error?.step === 'authorization_expired') clearStartAuthorization();
    if (request.action === 'destroy' && error?.step !== 'authorization_expired') return { action: request.action, value: failedDeletion(error, request.discardData, { storage, projectId: request.projectId, installationId: request.installationId }), ...(includeSelection ? { selection: { projectId: request.projectId, billingAccountName: request.billingAccountName } } : {}) };
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
  if (new URLSearchParams(fragment.replace(/^#/, '')).has('pause_ticket')) {
    pendingPause = parsePauseFragment(fragment);
    if (!authorization || authorization.clientId !== options.clientId || authorization.origin !== (options.location ?? window.location).origin || Date.now() >= authorization.expiresAt) savePauseForRedirect(pendingPause, options.session ?? sessionStorage);
    return { kind: 'pause_ready', selection: { projectId: pendingPause.projectId, installationId: pendingPause.installationId } };
  }
  try {
    const { action, value, selection } = await resumeFromRedirect({ ...options, fragment, includeSelection: true });
    return screenResult(action, value, selection);
  } catch (error) {
    if (error?.step !== 'authorization_expired') throw error;
    if (pendingPause) { savePauseForRedirect(pendingPause, options.session ?? sessionStorage); pendingPause = null; }
    retryFromGesture(options);
    return { kind: 'redirecting' };
  }
}

function screenResult(action, value, selection) {
  if (action === 'projects') return { kind: action, projects: value, selection };
  if (action === 'billing') return { kind: action, billingAccounts: value, selection };
  if (action === 'destroy' && value?.outcome === 'failed') throw new SetupError(value.reason ?? 'delete_failed');
  if (action === 'destroy' && value?.outcome === 'deleted_unverified') {
    return { kind: action, result: { ...value, remaining: [...value.remaining, ...value.unverified.map(name => 'unverified:' + name)] }, selection };
  }
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
