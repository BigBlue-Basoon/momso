import assert from 'node:assert/strict';
import { webcrypto } from 'node:crypto';
import test from 'node:test';
import { cancelPauseFromGesture, clearStartAuthorization, destroyFromGesture, projectsFromGesture, resumeAfterRedirect } from './momso_anywhere_start.mjs';
import { GoogleBrowserProvider } from './momso_anywhere_google_browser.mjs';

const id = 'a'.repeat(24), projectId = 'fixture-project', clientId = 'fixture.apps.googleusercontent.com';
const store = () => { const data = new Map(); return { data, getItem: k => data.get(k) ?? null, setItem: (k,v) => data.set(k,v), removeItem: k => data.delete(k) }; };

async function scenario(kind) {
  clearStartAuthorization();
  const session = store(), storage = store(), calls = [];
  const location = { href: 'https://login.example.test/start/', origin: 'https://login.example.test', assign(url) { this.next = url; } };
  const provider = { pauseGateway: async () => 'https://fixture.run.app', destroy: async (_id, _progress, { onVmState }) => {
    calls.push('destroy');
    if (kind === 'cancel') { assert.deepEqual(await cancelPauseFromGesture({ session }), { ok: true, pending: true }); onVmState('unknown'); }
    if (kind === 'failure') { onVmState('failed'); throw Error('fixture-failed'); }
    if (kind === 'unknown') { onVmState('unknown'); throw Error('fixture-unknown'); }
    onVmState('deleted'); return { remaining: [], mode: 'pause', status: 'complete' };
  } };
  const fetchImpl = async (url, options) => {
    calls.push(url.endsWith('/confirm') ? 'confirm' : 'release');
    assert.equal(options.credentials, 'omit');
    if (kind === 'cancel-confirm' && url.endsWith('/confirm')) assert.deepEqual(await cancelPauseFromGesture({ session }), { ok: true, pending: true });
    return { ok: kind !== 'confirm-failure' || !url.endsWith('/confirm'), json: async () => url.endsWith('/confirm') ? (kind === 'confirm-failure' ? { reason: 'invalid_pause_ticket' } : { ok: true, releaseTicket: 'b'.repeat(43) }) : { ok: true } };
  };
  const options = { clientId, release: {}, session, storage, location, cryptoImpl: webcrypto, fetchImpl, providerFactory: async () => ({ provider, ownerSub: '12345' }) };
  const now = Date.now();
  const pauseFragment = '#' + new URLSearchParams({ pause_ticket: 'a'.repeat(43), gateway: 'https://fixture.run.app', project_id: projectId, installation_id: id, created_at: String(now), expires_at: String(now + 60000) });
  assert.equal((await resumeAfterRedirect({ ...options, fragment: pauseFragment })).kind, 'pause_ready');
  assert.deepEqual(await destroyFromGesture({ ...options, projectId, installationId: id }), { redirecting: true });
  assert([...session.data.values()].some(v => v.includes('a'.repeat(43))));
  const state = new URL(location.next).searchParams.get('state');
  assert(!location.next.includes('a'.repeat(43)));
  const fragment = new URLSearchParams({ state, access_token: 'fixture-token', token_type: 'Bearer', expires_in: '60', scope: 'openid email https://www.googleapis.com/auth/cloud-platform' }).toString();
  let result, error;
  try { result = await resumeAfterRedirect({ ...options, fragment }); } catch (e) { error = e; }
  assert(!JSON.stringify([...session.data]).includes('a'.repeat(43)));
  return { calls, result, error };
}

test('confirms before deletion after a redirect', async () => {
  const r = await scenario('success'); assert.deepEqual(r.calls, ['confirm', 'destroy']); assert.equal(r.result.result.mode, 'pause');
  assert.equal(r.result.result.outcome, 'deleted_unverified');
  assert.deepEqual(r.result.result.remaining, ['unverified:resources']);
});
test('failed confirmation cannot start deletion', async () => {
  const r = await scenario('confirm-failure'); assert.deepEqual(r.calls, ['confirm']); assert.equal(r.error.step, 'invalid_pause_ticket');
});
test('confirmed deletion failure releases the gate', async () => {
  const r = await scenario('failure'); assert.deepEqual(r.calls, ['confirm', 'destroy', 'release']); assert.equal(r.error.step, 'delete_failed');
});
test('unknown deletion outcome never releases the gate', async () => {
  const r = await scenario('unknown'); assert.deepEqual(r.calls, ['confirm', 'destroy']); assert.equal(r.error.step, 'delete_failed');
});
test('cancellation before a VM request releases the gate', async () => {
  const r = await scenario('cancel'); assert.deepEqual(r.calls, ['confirm', 'destroy', 'release']); assert.equal(r.error.step, 'pause_cancelled');
});
test('cancellation while confirming releases without starting deletion', async () => {
  const r = await scenario('cancel-confirm'); assert.deepEqual(r.calls, ['confirm', 'release']); assert.equal(r.error.step, 'pause_cancelled');
});
test('cancel before confirmation releases and removes the temporary ticket', async () => {
  clearStartAuthorization();
  const session = store(), now = Date.now(), calls = [];
  const location = { href: 'https://login.example.test/start/', origin: 'https://login.example.test' };
  const fragment = '#' + new URLSearchParams({ pause_ticket: 'a'.repeat(43), gateway: 'https://fixture.run.app', project_id: projectId, installation_id: id, created_at: String(now), expires_at: String(now + 60000) });
  await resumeAfterRedirect({ clientId, session, location, fragment });
  assert.equal(session.data.size, 1);
  await cancelPauseFromGesture({ session, fetchImpl: async (url, options) => { calls.push([url, options.credentials]); return { ok: true, json: async () => ({ ok: true }) }; } });
  assert.deepEqual(calls, [['https://fixture.run.app/__momso/pause/release', 'omit']]);
  assert.equal(session.data.size, 0);
});
test('existing authorization keeps a new pause ticket only in memory', async () => {
  clearStartAuthorization();
  const session = store(), storage = store(), now = Date.now();
  const location = { href: 'https://login.example.test/start/', origin: 'https://login.example.test', assign(url) { this.next = url; } };
  const options = { clientId, session, storage, location, cryptoImpl: webcrypto, providerFactory: async () => ({ provider: { projects: async () => [] }, ownerSub: '12345' }) };
  await projectsFromGesture(options);
  const state = new URL(location.next).searchParams.get('state');
  await resumeAfterRedirect({ ...options, fragment: new URLSearchParams({ state, access_token: 'fixture-token', token_type: 'Bearer', expires_in: '60', scope: 'openid email https://www.googleapis.com/auth/cloud-platform' }).toString() });
  const fragment = '#' + new URLSearchParams({ pause_ticket: 'a'.repeat(43), gateway: 'https://fixture.run.app', project_id: projectId, installation_id: id, created_at: String(now), expires_at: String(now + 60000) });
  await resumeAfterRedirect({ ...options, fragment });
  assert(!JSON.stringify([...session.data, ...storage.data]).includes('a'.repeat(43)));
  await cancelPauseFromGesture({ session, fetchImpl: async () => ({ ok: true, json: async () => ({ ok: true }) }) });
});
test('gateway must match the selected project, installation and Google owner', async () => {
  const env = { MOMSO_PUBLIC_HOST: 'fixture.run.app', MOMSO_OWNER_SUB: '12345', MOMSO_GOOGLE_CLIENT_ID: clientId, MOMSO_PROJECT: projectId, MOMSO_INSTANCE: `momso-${id.slice(0, 12)}` };
  const service = { name: `projects/${projectId}/locations/asia-northeast3/services/momso-any-${id.slice(0, 12)}`, labels: { 'momso-install-id': id }, uri: 'https://fixture.run.app', template: { containers: [{ env: Object.entries(env).map(([name,value]) => ({ name, value })) }] } };
  const provider = new GoogleBrowserProvider('fixture-token', { projectId, clientId, ownerSub: '12345', fetchImpl: async () => ({ ok: true, status: 200, json: async () => service }) });
  assert.equal(await provider.pauseGateway(id), service.uri);
  service.labels['momso-install-id'] = 'b'.repeat(24);
  await assert.rejects(provider.pauseGateway(id), { step: 'resource_ownership' });
});
test('failure to start authorization releases and clears the pending ticket', async () => {
  clearStartAuthorization();
  const session = store(), now = Date.now(), calls = [];
  const location = { href: 'https://login.example.test/start/', origin: 'https://login.example.test', assign() {} };
  const options = { clientId: 'invalid', session, storage: store(), location, cryptoImpl: webcrypto,
    fetchImpl: async url => { calls.push(url); return { ok: true, json: async () => ({ ok: true }) }; } };
  const fragment = '#' + new URLSearchParams({ pause_ticket: 'a'.repeat(43), gateway: 'https://fixture.run.app', project_id: projectId, installation_id: id, created_at: String(now), expires_at: String(now + 60000) });
  await resumeAfterRedirect({ ...options, fragment });
  assert.equal((await destroyFromGesture({ ...options, projectId, installationId: id })).reason, 'invalid_authorization');
  assert.deepEqual(calls, ['https://fixture.run.app/__momso/pause/release']);
  assert.equal(session.data.size, 0);
});
