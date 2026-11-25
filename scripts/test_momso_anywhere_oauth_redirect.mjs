import assert from 'node:assert/strict';
import { webcrypto } from 'node:crypto';
import { spawnSync } from 'node:child_process';
import test, { beforeEach } from 'node:test';
import { beginRedirect, consumeRedirect } from './momso_anywhere_oauth_redirect.mjs';
import { billingAccountsFromGesture, clearStartAuthorization, destroyFromGesture, findFromGesture, projectsFromGesture, resumeAfterRedirect, resumeFromRedirect, retryFromGesture, retryLastAction, startFromGesture, watchGoogleSetupReturn } from './momso_anywhere_start.mjs';

beforeEach(clearStartAuthorization);

const memory = () => { const data = new Map(); return { getItem: k => data.get(k) ?? null, setItem: (k, v) => data.set(k, v), removeItem: k => data.delete(k), data }; };
const clientId = 'fixture.apps.googleusercontent.com';
const scopes = 'openid https://www.googleapis.com/auth/userinfo.email https://www.googleapis.com/auth/cloud-platform';

test('Google setup return checks focus and timer, waits for billing, and stops after success', async () => {
  const session = memory(), storage = memory();
  const location = { href: 'https://login.example.test/start/', origin: 'https://login.example.test', assign(url) { this.next = url; } };
  let ready = false;
  const provider = { billingAccounts: async () => ready ? [{ name: 'billingAccounts/ABC-123' }] : [] };
  const options = { clientId, release: {}, session, storage, location, cryptoImpl: webcrypto, providerFactory: async () => ({ provider, ownerSub: '12345' }) };
  beginRedirect({ clientId, action: 'billing', storage: session, location, cryptoImpl: webcrypto });
  const fragment = new URLSearchParams({ state: new URL(location.next).searchParams.get('state'), access_token: 'fixture-token', token_type: 'Bearer', expires_in: '60', scope: scopes }).toString();
  await resumeFromRedirect({ ...options, fragment });
  session.setItem('momso.start.retry.v1', JSON.stringify({ action: 'billing' }));
  const events = new Map();
  const doc = { visibilityState: 'visible', addEventListener: (n, f) => events.set(n, f), removeEventListener: n => events.delete(n) };
  const win = { addEventListener: (n, f) => events.set(n, f), removeEventListener: n => events.delete(n), setInterval: f => { events.set('timer', f); return 1; }, clearInterval: () => events.delete('timer') };
  let result;
  const watch = watchGoogleSetupReturn({ ...options, documentImpl: doc, windowImpl: win, onResult: value => { result = value; } });
  await events.get('timer')();
  assert.equal(result, undefined);
  await events.get('focus')();
  assert.equal(result, undefined);
  ready = true;
  await events.get('visibilitychange')();
  assert.equal(result.billingAccounts.length, 1);
  assert.equal(events.size, 0);
  watch.stop();
  assert(!JSON.stringify([...session.data]).includes('fixture-token'));
});

test('capture removes fragment immediately and only retains it in module memory', () => {
  const source = `globalThis.window={location:{hash:'#access_token=fixture',pathname:'/start/',search:''},history:{replaceState(_a,_b,p){globalThis.cleaned=p}}}; const m=await import('./scripts/momso_anywhere_oauth_capture.mjs'); if(cleaned!=='/start/'||!m.hasOAuthFragment()||m.takeOAuthFragment()!=='#access_token=fixture'||m.hasOAuthFragment())process.exit(1);`;
  const result = spawnSync(process.execPath, ['--input-type=module', '-e', source], { encoding: 'utf8' });
  assert.equal(result.status, 0, result.stderr);
});

test('pause fragment is removed from the address before screen code runs', () => {
  const source = `globalThis.window={location:{hash:'#pause_ticket=fixture',pathname:'/start/',search:''},history:{replaceState(_a,_b,p){globalThis.cleaned=p}}}; const m=await import('./scripts/momso_anywhere_oauth_capture.mjs'); if(cleaned!=='/start/'||m.takeOAuthFragment()!=='#pause_ticket=fixture'||m.takeOAuthFragment()!=='')process.exit(1);`;
  const result = spawnSync(process.execPath, ['--input-type=module', '-e', source], { encoding: 'utf8' });
  assert.equal(result.status, 0, result.stderr);
});

test('whole-page Google redirect validates state and retains token only in memory', () => {
  const storage = memory();
  const location = { href: 'https://login.example.test/start/', assign(url) { this.next = url; } };
  assert.deepEqual(beginRedirect({ clientId, action: 'start', projectId: 'fixture-project', storage, location, cryptoImpl: webcrypto, now: 1000 }), { redirecting: true });
  const auth = new URL(location.next);
  assert.equal(auth.searchParams.get('response_type'), 'token');
  assert.equal(auth.searchParams.get('redirect_uri'), 'https://login.example.test/start/');
  const fragment = new URLSearchParams({ state: auth.searchParams.get('state'), access_token: 'fixture-token', token_type: 'Bearer', expires_in: '3600', scope: scopes }).toString();
  const result = consumeRedirect({ fragment, storage, origin: 'https://login.example.test', clientId, now: 2000 });
  assert.equal(result.projectId, 'fixture-project');
  assert.equal(result.token, 'fixture-token');
  assert(!JSON.stringify([...storage.data]).includes('fixture-token'));
  assert.throws(() => consumeRedirect({ fragment, storage, origin: 'https://login.example.test', clientId, now: 2000 }), { step: 'authorization_invalid' });
});

test('rejects mismatched state, missing scopes and expired authorization request', () => {
  for (const kind of ['state', 'scope', 'time']) {
    const storage = memory();
    const location = { href: 'https://login.example.test/start/', assign(url) { this.next = url; } };
    beginRedirect({ clientId, action: 'find', storage, location, cryptoImpl: webcrypto, now: 1000 });
    const state = new URL(location.next).searchParams.get('state');
    const fragment = new URLSearchParams({ state: kind === 'state' ? 'wrong' : state, access_token: 'fixture', token_type: 'Bearer', expires_in: '60', scope: kind === 'scope' ? 'openid email' : scopes }).toString();
    assert.throws(() => consumeRedirect({ fragment, storage, origin: 'https://login.example.test', clientId, now: kind === 'time' ? 1000000 : 2000 }), { step: 'authorization_invalid' });
  }
});

test('redirect return resumes a project request and retains retry without storing the token', async () => {
  const session = memory(), storage = memory();
  const location = { href: 'https://login.example.test/start/', origin: 'https://login.example.test', assign(url) { this.next = url; } };
  beginRedirect({ clientId, action: 'projects', storage: session, location, cryptoImpl: webcrypto });
  const state = new URL(location.next).searchParams.get('state');
  const fragment = new URLSearchParams({ state, access_token: 'fixture-private-token', token_type: 'Bearer', expires_in: '60', scope: scopes }).toString();
  await assert.rejects(resumeFromRedirect({ clientId, release: {}, session, storage, location, cryptoImpl: webcrypto, fragment, fetchImpl: async () => ({ ok: false, status: 401 }) }), { step: 'authorization_expired' });
  assert(!JSON.stringify([...session.data]).includes('fixture-private-token'));
  retryFromGesture({ clientId, session, location, cryptoImpl: webcrypto });
  const retry = new URL(location.next);
  const nextFragment = new URLSearchParams({ state: retry.searchParams.get('state'), access_token: 'fixture-private-token', token_type: 'Bearer', expires_in: '60', scope: scopes }).toString();
  const result = await resumeFromRedirect({ clientId, release: {}, session, storage, location, cryptoImpl: webcrypto, fragment: nextFragment, fetchImpl: async url => ({ ok: true, json: async () => url.includes('userinfo') ? { sub: '12345' } : { projects: [{ projectId: 'fixture-project', state: 'ACTIVE' }] } }) });
  assert.deepEqual(result, { action: 'projects', value: [{ projectId: 'fixture-project', displayName: '' }] });
  assert(!JSON.stringify([...session.data]).includes('fixture-private-token'));
});

test('expired token retries the same installation and selected project', async () => {
  const session = memory(), storage = memory(), ids = [];
  const location = { href: 'https://login.example.test/start/', origin: 'https://login.example.test', assign(url) { this.next = url; } };
  const steps = ['project', 'billing', 'services', 'privateNetwork', 'server', 'gateway', 'gatewayReady'];
  let expired = true;
  const provider = Object.fromEntries(steps.map(step => [step, async id => { ids.push(id); if (step === 'server' && expired) { expired = false; const e = new Error('expired'); e.step = 'authorization_expired'; throw e; } }]));
  // A real provider reports its errors as SetupError; emulate the same contract.
  const { SetupError } = await import('./momso_anywhere_setup.mjs');
  provider.server = async id => { ids.push(id); if (expired) { expired = false; throw new SetupError('authorization_expired'); } };
  provider.preflight = async () => {};
  provider.gatewayUrl = () => 'https://fixture.run.app';
  const auth = () => new URLSearchParams({ state: new URL(location.next).searchParams.get('state'), access_token: 'fixture-token', token_type: 'Bearer', expires_in: '60', scope: scopes }).toString();
  beginRedirect({ clientId, action: 'start', projectId: 'fixture-project', storage: session, location, cryptoImpl: webcrypto });
  await assert.rejects(resumeFromRedirect({ clientId, release: {}, session, storage, location, cryptoImpl: webcrypto, fragment: auth(), providerFactory: async () => ({ provider, ownerSub: '12345' }) }), { step: 'authorization_expired' });
  retryFromGesture({ clientId, session, location, cryptoImpl: webcrypto });
  const result = await resumeFromRedirect({ clientId, release: {}, session, storage, location, cryptoImpl: webcrypto, fragment: auth(), providerFactory: async request => { assert.equal(request.projectId, 'fixture-project'); return { provider, ownerSub: '12345' }; } });
  assert.equal(result.value.url, 'https://fixture.run.app');
  assert.equal(new Set(ids).size, 1);
  assert(!JSON.stringify([...storage.data, ...session.data]).includes('fixture-token'));
});

test('screen resumes lists, selections and installation through full-page returns', async () => {
  const session = memory(), storage = memory(), ids = [], progress = [];
  const location = { href: 'https://login.example.test/start/', origin: 'https://login.example.test', assign(url) { this.next = url; } };
  const selection = { projectId: 'fixture-project', billingAccountName: 'billingAccounts/ABC-123' };
  const steps = ['project', 'billing', 'services', 'privateNetwork', 'server', 'gateway', 'gatewayReady'];
  const provider = Object.fromEntries(steps.map(step => [step, async id => { ids.push(id); }]));
  provider.preflight = async () => {};
  provider.gatewayUrl = () => 'https://fixture.run.app';
  provider.projects = async () => [{ projectId: selection.projectId }];
  provider.billingAccounts = async () => [{ name: selection.billingAccountName }];
  const base = { clientId, release: {}, session, storage, location, cryptoImpl: webcrypto, providerFactory: async request => {
    assert([null, selection.projectId].includes(request.projectId));
    assert([null, selection.billingAccountName].includes(request.billingAccountName));
    return { provider, ownerSub: '12345' };
  } };
  const fragment = () => new URLSearchParams({ state: new URL(location.next).searchParams.get('state'), access_token: 'fixture-token', token_type: 'Bearer', expires_in: '60', scope: scopes }).toString();
  const resume = () => resumeAfterRedirect({ ...base, fragment: fragment(), onProgress: item => progress.push(item) });
  assert.equal(await resumeAfterRedirect({ ...base, fragment: '' }), null);
  await projectsFromGesture(base);
  assert.deepEqual(await resume(), { kind: 'projects', projects: [{ projectId: selection.projectId }], selection: { projectId: null, billingAccountName: null } });
  const firstDestination = location.next;
  assert.deepEqual(await billingAccountsFromGesture({ ...base, ...selection }), [{ name: selection.billingAccountName }]);
  const result = await startFromGesture({ ...base, ...selection, onProgress: item => progress.push(item) });
  assert.equal(result.url, 'https://fixture.run.app');
  assert.equal(location.next, firstDestination);
  assert(progress.length > 0);
  assert.equal(new Set(ids).size, 1);
  assert(!JSON.stringify([...storage.data, ...session.data]).includes('fixture-token'));
});

test('the cached token expires before the next gesture and redirects only then', async t => {
  let now = Date.now();
  t.mock.method(Date, 'now', () => now);
  const session = memory(), storage = memory();
  const location = { href: 'https://login.example.test/start/', origin: 'https://login.example.test', calls: 0, assign(url) { this.next = url; this.calls++; } };
  const providerFactory = async () => ({ provider: { projects: async () => [], billingAccounts: async () => [] }, ownerSub: '12345' });
  const base = { clientId, release: {}, session, storage, location, cryptoImpl: webcrypto, providerFactory };
  await projectsFromGesture(base);
  const fragment = new URLSearchParams({ state: new URL(location.next).searchParams.get('state'), access_token: 'fixture-token', token_type: 'Bearer', expires_in: '60', scope: scopes }).toString();
  await resumeAfterRedirect({ ...base, fragment });
  assert.deepEqual(await billingAccountsFromGesture(base), []);
  assert.equal(location.calls, 1);
  now += 60001;
  assert.deepEqual(await billingAccountsFromGesture({ ...base, projectId: 'fixture-project' }), { redirecting: true });
  assert.equal(location.calls, 2);
  assert.equal(JSON.parse(session.getItem('momso.start.oauth.pending.v1')).projectId, 'fixture-project');
  assert(!JSON.stringify([...session.data, ...storage.data]).includes('fixture-token'));
});

test('destroy is blocked before provider mutation when backup freshness is unverified', async () => {
  const session = memory(), storage = memory();
  const location = { href: 'https://login.example.test/start/', origin: 'https://login.example.test', calls: 0, assign(url) { this.next = url; this.calls++; } };
  let deletions = 0;
  const providerFactory = async () => ({ provider: { projects: async () => [], findExisting: async () => ['https://fixture.run.app'], destroy: async () => { deletions++; return { remaining: [] }; } }, ownerSub: '12345' });
  const base = { clientId, release: {}, session, storage, location, cryptoImpl: webcrypto, providerFactory };
  await projectsFromGesture(base);
  const fragment = new URLSearchParams({ state: new URL(location.next).searchParams.get('state'), access_token: 'fixture-token', token_type: 'Bearer', expires_in: '60', scope: scopes }).toString();
  await resumeAfterRedirect({ ...base, fragment });
  assert.deepEqual(await findFromGesture(base), ['https://fixture.run.app']);
  assert.equal((await destroyFromGesture({ ...base, projectId: 'fixture-project', installationId: 'a'.repeat(24) })).reason, 'pause_requires_app_session');
  assert.equal(deletions, 0);
  assert.equal(location.calls, 1);
  assert(!JSON.stringify([...session.data, ...storage.data]).includes('fixture-token'));
});

test('revoked cached authorization redirects the same action without retrying the token', async () => {
  const session = memory(), storage = memory();
  const location = { href: 'https://login.example.test/start/', origin: 'https://login.example.test', calls: 0, assign(url) { this.next = url; this.calls++; } };
  const { SetupError } = await import('./momso_anywhere_setup.mjs');
  let calls = 0;
  const providerFactory = async () => ({ provider: { projects: async () => [], billingAccounts: async () => { calls++; throw new SetupError('authorization_expired'); } }, ownerSub: '12345' });
  const base = { clientId, release: {}, session, storage, location, cryptoImpl: webcrypto, providerFactory };
  await projectsFromGesture(base);
  const fragment = new URLSearchParams({ state: new URL(location.next).searchParams.get('state'), access_token: 'fixture-token', token_type: 'Bearer', expires_in: '60', scope: scopes }).toString();
  await resumeAfterRedirect({ ...base, fragment });
  assert.deepEqual(await billingAccountsFromGesture({ ...base, projectId: 'fixture-project' }), { redirecting: true });
  assert.equal(calls, 1);
  assert.equal(location.calls, 2);
  assert.equal(JSON.parse(session.getItem('momso.start.oauth.pending.v1')).projectId, 'fixture-project');
});

test('retry after a transient error uses the still-valid token', async () => {
  const session = memory(), storage = memory();
  const location = { href: 'https://login.example.test/start/', origin: 'https://login.example.test', calls: 0, assign(url) { this.next = url; this.calls++; } };
  const { SetupError } = await import('./momso_anywhere_setup.mjs');
  let failed = false;
  const providerFactory = async () => ({ provider: { projects: async () => [], billingAccounts: async () => { if (!failed) { failed = true; throw new SetupError('network'); } return [{ name: 'billingAccounts/ABC-123' }]; } }, ownerSub: '12345' });
  const base = { clientId, release: {}, session, storage, location, cryptoImpl: webcrypto, providerFactory };
  await projectsFromGesture(base);
  const fragment = new URLSearchParams({ state: new URL(location.next).searchParams.get('state'), access_token: 'fixture-token', token_type: 'Bearer', expires_in: '60', scope: scopes }).toString();
  await resumeAfterRedirect({ ...base, fragment });
  await assert.rejects(billingAccountsFromGesture({ ...base, projectId: 'fixture-project' }), { step: 'network' });
  assert.deepEqual(await retryLastAction(base), { kind: 'billing', billingAccounts: [{ name: 'billingAccounts/ABC-123' }], selection: { projectId: 'fixture-project', billingAccountName: null } });
  assert.equal(location.calls, 1);
});

test('cached authorization is not reused for another client or after clearing', async () => {
  const session = memory(), storage = memory();
  const location = { href: 'https://login.example.test/start/', origin: 'https://login.example.test', calls: 0, assign(url) { this.next = url; this.calls++; } };
  let providerCalls = 0;
  const providerFactory = async () => { providerCalls++; return { provider: { projects: async () => [] }, ownerSub: '12345' }; };
  const base = { clientId, release: {}, session, storage, location, cryptoImpl: webcrypto, providerFactory };
  await projectsFromGesture(base);
  const fragment = new URLSearchParams({ state: new URL(location.next).searchParams.get('state'), access_token: 'fixture-token', token_type: 'Bearer', expires_in: '60', scope: scopes }).toString();
  await resumeAfterRedirect({ ...base, fragment });
  assert.equal(providerCalls, 1);
  assert.deepEqual(await projectsFromGesture({ ...base, clientId: 'other.apps.googleusercontent.com' }), { redirecting: true });
  assert.equal(providerCalls, 1);
  clearStartAuthorization();
  assert.deepEqual(await projectsFromGesture(base), { redirecting: true });
  assert.equal(providerCalls, 1);
});

test('screen resumes expired authorization with the same selection', async () => {
  const session = memory(), storage = memory();
  const location = { href: 'https://login.example.test/start/', origin: 'https://login.example.test', assign(url) { this.next = url; } };
  const base = { clientId, release: {}, session, storage, location, cryptoImpl: webcrypto };
  await projectsFromGesture({ ...base, projectId: 'fixture-project', billingAccountName: 'billingAccounts/ABC-123' });
  const fragment = new URLSearchParams({ state: new URL(location.next).searchParams.get('state'), access_token: 'fixture-token', token_type: 'Bearer', expires_in: '60', scope: scopes }).toString();
  const result = await resumeAfterRedirect({ ...base, fragment, fetchImpl: async () => ({ ok: false, status: 401 }) });
  assert.deepEqual(result, { kind: 'redirecting' });
  const pending = JSON.parse(session.getItem('momso.start.oauth.pending.v1'));
  assert.equal(pending.projectId, 'fixture-project');
  assert.equal(pending.billingAccountName, 'billingAccounts/ABC-123');
  assert(!JSON.stringify([...session.data]).includes('fixture-token'));
});

test('screen automatically retries an installation when authorization expires mid-step', async () => {
  const session = memory(), storage = memory(), ids = [];
  const location = { href: 'https://login.example.test/start/', origin: 'https://login.example.test', assign(url) { this.next = url; } };
  const base = { clientId, release: {}, session, storage, location, cryptoImpl: webcrypto };
  const { SetupError } = await import('./momso_anywhere_setup.mjs');
  let expired = false;
  const provider = Object.fromEntries(['project', 'billing', 'services', 'privateNetwork', 'server', 'gateway', 'gatewayReady'].map(step => [step, async id => {
    ids.push(id);
    if (step === 'server' && !expired) { expired = true; throw new SetupError('authorization_expired'); }
  }]));
  provider.preflight = async () => {};
  provider.gatewayUrl = () => 'https://fixture.run.app';
  const fragment = () => new URLSearchParams({ state: new URL(location.next).searchParams.get('state'), access_token: 'fixture-token', token_type: 'Bearer', expires_in: '60', scope: scopes }).toString();
  const resume = () => resumeAfterRedirect({ ...base, fragment: fragment(), providerFactory: async request => {
    assert.equal(request.projectId, 'fixture-project');
    assert.equal(request.billingAccountName, 'billingAccounts/ABC-123');
    return { provider, ownerSub: '12345' };
  } });
  await startFromGesture({ ...base, projectId: 'fixture-project', billingAccountName: 'billingAccounts/ABC-123' });
  assert.deepEqual(await resume(), { kind: 'redirecting' });
  assert.equal((await resume()).result.url, 'https://fixture.run.app');
  assert.equal(new Set(ids).size, 1);
  assert(!JSON.stringify([...storage.data, ...session.data]).includes('fixture-token'));
});
