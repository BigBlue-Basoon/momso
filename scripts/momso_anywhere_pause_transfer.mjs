import { SetupError } from './momso_anywhere_setup.mjs';

const KEY = 'momso.start.pause.v1';

function valid(value, now) {
  if (!value || typeof value !== 'object' || !/^[A-Za-z0-9_-]{32,128}$/.test(value.ticket ?? '') ||
      !/^[a-f0-9]{24}$/.test(value.installationId ?? '') || !/^[a-z][a-z0-9-]{4,28}[a-z0-9]$/.test(value.projectId ?? '') ||
      !Number.isSafeInteger(value.createdAt) || !Number.isSafeInteger(value.expiresAt) ||
      value.createdAt > now || value.expiresAt <= now || value.expiresAt <= value.createdAt || value.expiresAt - value.createdAt > 300000) return false;
  try {
    const url = new URL(value.gateway);
    return url.protocol === 'https:' && url.origin === value.gateway && url.hostname.endsWith('.run.app') && !url.username && !url.password;
  } catch { return false; }
}

export function parsePauseFragment(fragment, now = Date.now()) {
  const params = new URLSearchParams((fragment || '').replace(/^#/, ''));
  const keys = ['pause_ticket', 'gateway', 'project_id', 'installation_id', 'created_at', 'expires_at'];
  if (keys.some(key => params.getAll(key).length !== 1) || [...params.keys()].some(key => !keys.includes(key)) ||
      !/^\d{1,16}$/.test(params.get('created_at')) || !/^\d{1,16}$/.test(params.get('expires_at'))) throw new SetupError('invalid_pause_ticket');
  const value = { ticket: params.get('pause_ticket'), gateway: params.get('gateway'), projectId: params.get('project_id'),
    installationId: params.get('installation_id'), createdAt: Number(params.get('created_at')), expiresAt: Number(params.get('expires_at')) };
  if (!valid(value, now)) throw new SetupError('invalid_pause_ticket');
  return value;
}

export function savePauseForRedirect(value, session, now = Date.now()) {
  if (!valid(value, now)) throw new SetupError('invalid_pause_ticket');
  try { session.setItem(KEY, JSON.stringify(value)); }
  catch { throw new SetupError('checkpoint_unavailable'); }
}

export function takePauseAfterRedirect(session, now = Date.now()) {
  let raw;
  try { raw = session.getItem(KEY); session.removeItem(KEY); }
  catch { throw new SetupError('checkpoint_unavailable'); }
  if (!raw) return null;
  let value;
  try { value = JSON.parse(raw); } catch { throw new SetupError('invalid_pause_ticket'); }
  if (!valid(value, now)) throw new SetupError('invalid_pause_ticket');
  return value;
}

export function clearPause(session) {
  try { session.removeItem(KEY); } catch { throw new SetupError('checkpoint_unavailable'); }
}
