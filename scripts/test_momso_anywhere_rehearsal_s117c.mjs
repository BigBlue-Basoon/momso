import assert from 'node:assert/strict';
import test from 'node:test';
import { accessToken, cleanupBackup, discoverProtectedInstallations, executeBrowserRehearsal, guardedFetch, interruptGatewayOnce, runBrowserTrial, validateScope, verifyBrowserInstallation } from './momso_anywhere_rehearsal_s117c.mjs';
import { signedFixture } from './test_momso_release_fixture.mjs';

const projectId = 'fixture-project', installationId = 'a'.repeat(24);
test('automatic protection discovers VM and gateway names and fails on denied inventory', async () => {
  const fetcher = async raw => {
    const url = new URL(raw);
    if (url.hostname === 'compute.googleapis.com') return Response.json({ items: { 'zones/example': { instances: [{ name: 'momso-bbbbbbbbbbbb' }] } } });
    return Response.json({ items: [{ metadata: { name: 'momso-any-cccccccccccc' } }] });
  };
  assert.deepEqual(await discoverProtectedInstallations(projectId, 'fixture-token', fetcher), ['bbbbbbbbbbbb', 'cccccccccccc']);
  await assert.rejects(discoverProtectedInstallations(projectId, 'fixture-token', async () => new Response('', { status: 403 })), /protected_inventory_unavailable/);
});
test('automatic protection rejects a matching installation before start or destroy', async () => {
  const fetcher = async raw => {
    const url = new URL(raw);
    return url.hostname === 'compute.googleapis.com'
      ? Response.json({ items: { 'zones/example': { instances: [{ name: 'momso-' + installationId.slice(0, 12) }] } } })
      : Response.json({ items: [] });
  };
  await assert.rejects(executeBrowserRehearsal({ projectId, installationId, clientId: 'fixture.apps.googleusercontent.com', release: {}, token: 'fixture-token', fetchImpl: fetcher, providerFactory: () => { throw new Error('must not create provider'); } }), /invalid_scope/);
});
test('protected installation is rejected before any callbacks', async () => {
  const calls = [];
  await assert.rejects(runBrowserTrial({ projectId, installationId: 'bbbbbbbbbbbb' + 'a'.repeat(12), protectedInstallationIds: ['bbbbbbbbbbbb'], start: () => calls.push(1), verify: () => {}, destroy: () => {}, inventory: () => {} }));
  assert.deepEqual(calls, []);
  assert.throws(() => validateScope('bad/project', installationId));
});

test('gateway interruption resumes same installation and always destroys', async () => {
  const calls = [], identities = [];
  let attempt = 0;
  const result = await runBrowserTrial({ projectId, installationId,
    start: async (ctx, interrupt) => { identities.push(ctx.installationId); calls.push('start'); if (interrupt && ++attempt === 1) throw Object.assign(new Error('secret'), { step: 'gateway' }); },
    verify: async () => ({ traffic_digest: 'passed', public_https: 'passed', vm_services: 'unverified', update_status: 'unverified' }),
    destroy: async () => { calls.push('destroy'); return { remaining: [] }; },
    inventory: async () => { calls.push('inventory'); return { vm: 'unknown' }; },
    backupCleanup: async () => { calls.push('backup'); return 'unknown'; },
  });
  assert.deepEqual(calls, ['start', 'start', 'destroy', 'backup', 'inventory']);
  assert.deepEqual(identities, [installationId, installationId]);
  assert.equal(result.interruption, 'passed');
  assert.equal(result.resume, 'passed');
  assert.equal(result.failure, 'verification_incomplete');
  assert(!JSON.stringify(result).includes('secret'));
});

test('failure before retry still invokes destruction and inventory', async () => {
  const calls = [];
  const result = await runBrowserTrial({ projectId, installationId,
    start: async () => { throw new Error('fixture-private'); }, verify: async () => ({}),
    destroy: async () => { calls.push('destroy'); return { remaining: ['network'] }; },
    inventory: async () => { calls.push('inventory'); return {}; } });
  assert.deepEqual(calls, ['destroy', 'inventory']);
  assert.equal(result.cleanup, 'incomplete');
  assert(!JSON.stringify(result).includes('fixture-private'));
});

test('interruption adapter delegates actual bound methods', async () => {
  const steps = ['project', 'billing', 'services', 'privateNetwork', 'server', 'gateway', 'gatewayReady'];
  const original = { value: 'same', preflight() {}, gatewayUrl() { return this.value; } };
  for (const name of steps) original[name] = function (id) { assert.equal(this, original); return id; };
  const wrapped = interruptGatewayOnce(original);
  await assert.rejects(wrapped.gateway(installationId));
  assert.equal(await wrapped.gateway(installationId), installationId);
  assert.equal(wrapped.gatewayUrl(), 'same');
});

test('actual traffic and public status do not invent private VM evidence', async () => {
  const digest = 'ghcr.io/example/test@sha256:' + '1'.repeat(64), calls = [];
  const fake = async (url, options) => {
    calls.push([url, options]);
    if (url.endsWith('/revisions/rev-1')) return new Response(JSON.stringify({ conditions: [{ type: 'Ready', state: 'CONDITION_SUCCEEDED' }], containers: [{ image: digest }] }), { status: 200 });
    if (url.endsWith('/__momso/ready')) return new Response('{}', { status: 200 });
    return new Response(JSON.stringify({ labels: { 'momso-install-id': installationId }, trafficStatuses: [{ revision: 'rev-1', percent: 100 }], uri: 'https://fixture.run.app' }), { status: 200 });
  };
  const result = await verifyBrowserInstallation({ projectId, installationId }, { token: 'fixture-token', signed: { gatewayImage: digest }, fetchImpl: fake });
  assert.deepEqual(result, { traffic_digest: 'passed', public_https: 'passed', vm_services: 'unverified', update_status: 'unverified' });
  assert.equal(calls.at(-1)[1].headers, undefined);
});

test('VM diagnostics are accepted only after temporary firewall removal', async () => {
  const signed = { gatewayImage: 'ghcr.io/example/test@sha256:' + '1'.repeat(64) };
  const fetchImpl = async () => new Response('{}', { status: 404 });
  const options = { account: 'fixture@example.test', token: 'fixture-token', signed, fetchImpl };
  const result = await verifyBrowserInstallation({ projectId, installationId }, { ...options, diagnostics: async () => ({ vm_services: 'passed', update_status: 'passed', diagnosticFirewall: 'removed' }) });
  assert.equal(result.vm_services, 'passed');
  assert.equal(result.update_status, 'passed');
  assert.equal(result.public_https, 'unverified');
  const denied = await verifyBrowserInstallation({ projectId, installationId }, { ...options, diagnostics: async () => ({ vm_services: 'passed', update_status: 'passed', diagnosticFirewall: 'remaining_or_unknown', diagnosticRule: 'fixture-rule' }) });
  assert.equal(denied.vm_services, 'unverified');
  assert.equal(denied.diagnosticRule, 'fixture-rule');
});

test('access token is captured rather than printed or passed in arguments', () => {
  const calls = [];
  const token = accessToken('person@example.test', (program, args, options) => { calls.push({ program, args, options }); return { status: 0, stdout: 'fixture-token\n' }; }, 'linux');
  assert.equal(token, 'fixture-token');
  assert(!JSON.stringify(calls).includes('fixture-token'));
});

test('Windows token retrieval runs gcloud Python without a shell', () => {
  const calls = [];
  const token = accessToken('person@example.test', (program, args, options) => {
    calls.push({ program, args, options });
    if (program === 'where.exe') return { status: 0, stdout: 'C:\\CloudSDK\\bin\\gcloud.cmd\r\n' };
    return { status: 0, stdout: 'fixture-token\n' };
  }, 'win32');
  assert.equal(token, 'fixture-token');
  assert.equal(calls.length, 2);
  assert.equal(calls[1].options.shell, undefined);
  assert.equal(calls[1].options.env.CLOUDSDK_CORE_LOG_HTTP, 'false');
  assert(calls[1].args.includes('--account=person@example.test'));
});

test('fetch guard refuses project deletion and protected installation changes', async () => {
  const calls = [];
  const safe = guardedFetch(async (...args) => { calls.push(args); return {}; }, projectId, ['bbbbbbbbbbbb']);
  await assert.rejects(safe(`https://cloudresourcemanager.googleapis.com/v3/projects/${projectId}`, { method: 'DELETE' }));
  await assert.rejects(safe('https://compute.googleapis.com/compute/v1/projects/fixture-project/zones/z/instances/momso-bbbbbbbbbbbb', { method: 'DELETE' }));
  await safe(`https://cloudresourcemanager.googleapis.com/v3/projects/${projectId}`, { method: 'GET' });
  assert.equal(calls.length, 1);
});

test('actual gesture modules retry and invoke destroy with a fake provider', async () => {
  const fixture = await signedFixture(), calls = [];
  const steps = ['project', 'billing', 'services', 'privateNetwork', 'server', 'gateway', 'gatewayReady'];
  const provider = {
    async preflight() {}, gatewayUrl() { return 'https://fixture.run.app'; },
    async projects() { return [{ projectId }]; },
    async destroy(id) { calls.push(['destroy', id]); return { remaining: [] }; },
  };
  for (const step of steps) provider[step] = async id => { calls.push([step, id]); };
  const providerFactory = async () => ({ ownerSub: '123456', provider });
  const fakeFetch = async raw => {
    const url = new URL(raw);
    if (url.hostname === 'fixture.test') return fixture.response(url.pathname.endsWith('.sig') ? fixture.signature : fixture.raw);
    return new Response('{}', { status: 404 });
  };
  const result = await executeBrowserRehearsal({ projectId, installationId, protectedInstallationIds: ['bbbbbbbbbbbb'], clientId: 'fake.apps.googleusercontent.com', release: fixture.release, token: 'fake-token', fetchImpl: fakeFetch, providerFactory });
  assert.equal(result.interruption, 'passed');
  assert.equal(result.resume, 'passed');
  assert.equal(result.cleanup, 'passed');
  assert.equal(result.failure, 'verification_incomplete');
  assert.equal(calls.filter(([step]) => step === 'project').length, 2);
  assert.equal(calls.filter(([step]) => step === 'gateway').length, 1);
  assert.deepEqual(calls.at(-1), ['destroy', installationId]);
  assert(calls.every(([, id]) => id === installationId));
});
