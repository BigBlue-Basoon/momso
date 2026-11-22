import { authorizationUrl, consumeGoogleReturn, needsFreshChallenge, prepareLogin, savedLoginContext, validateRedirectContext } from './momso_anywhere_login.mjs';

export async function bootLoginPage({ doc, win, fetchImpl = fetch, cryptoImpl = crypto }) {
  const url = new URL(win.location.href);
  const fragment = url.hash;
  // Clear credentials before parsing, fetching configuration or reporting errors.
  if (fragment) win.history.replaceState(null, '', url.pathname + url.search);
  const button = doc.getElementById('google-login');
  if (button) button.disabled = true;
  let context = null;
  let clientId = null;
  const loadClientId = async () => {
    const response = await fetchImpl('./config.json', { cache: 'no-store' });
    if (!response.ok) throw new Error('config_unavailable');
    const config = await response.json();
    if (typeof config.clientId !== 'string' || !config.clientId) throw new Error('config_unavailable');
    return config.clientId;
  };
  const error = () => {
    doc.body.dataset.state = 'error';
    if (button) button.disabled = false;
  };
  if (button) button.addEventListener('click', async () => {
    button.disabled = true;
    doc.body.dataset.state = 'working';
    try {
      if (!context) { win.location.reload(); return; }
      if (needsFreshChallenge(context.state)) { win.location.replace(context.origin + '/'); return; }
      clientId ||= await loadClientId();
      const destination = await authorizationUrl({ ...context, referrer: context.origin, clientId, pageOrigin: win.location.origin, storage: win.sessionStorage, cryptoImpl });
      win.location.assign(destination);
    } catch { error(); }
  });
  if (fragment) {
    try {
      const destination = consumeGoogleReturn({ fragment, storage: win.sessionStorage });
      doc.body.dataset.state = 'working';
      win.location.replace(destination);
    } catch { context = savedLoginContext(win.sessionStorage); error(); }
    return;
  }
  const states = url.searchParams.getAll('state');
  const state = states.length === 1 ? states[0] : null;
  try {
    validateRedirectContext(state, doc.referrer);
  } catch {
    doc.body.dataset.state = 'invalid';
    return;
  }
  try {
    context = prepareLogin({ state, referrer: doc.referrer, storage: win.sessionStorage });
    clientId = await loadClientId();
    if (!button) throw new Error('button_unavailable');
    doc.body.dataset.state = url.searchParams.get('error') === '1' ? 'error' : 'ready';
    button.disabled = false;
  } catch { error(); }
}

if (typeof window !== 'undefined' && typeof document !== 'undefined') {
  void bootLoginPage({ doc: document, win: window });
}
