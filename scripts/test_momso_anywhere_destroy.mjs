import assert from 'node:assert/strict';
import test from 'node:test';
import { webcrypto } from 'node:crypto';
import { spawnSync } from 'node:child_process';
import { mkdtemp, readFile, rm } from 'node:fs/promises';
import { tmpdir } from 'node:os';
import { join } from 'node:path';
import { GoogleBrowserProvider } from './momso_anywhere_google_browser.mjs';
import { executeDestroy } from './momso_anywhere_destroy_cli.mjs';
import { clearStartAuthorization, destroyFromGesture, resumeFromRedirect } from './momso_anywhere_start.mjs';
import { beginRedirect } from './momso_anywhere_oauth_redirect.mjs';

const id = 'a'.repeat(24), short = id.slice(0, 12), projectId = 'fixture-project', name = `momso-${short}`;
const marker = `momso-install-${id}`;
const ok = (value, status = 200) => ({ ok: status >= 200 && status < 300, status, json: async () => value });

function fixture({ retainVm = false, rejectVm = false, loseVmResponse = false, denyPolicy = false } = {}) {
  const c = `https://compute.googleapis.com/compute/v1/projects/${projectId}/`;
  const i = `https://iam.googleapis.com/v1/projects/${projectId}/`;
  const s = `https://secretmanager.googleapis.com/v1/projects/${projectId}/secrets/`;
  const r = `https://run.googleapis.com/v2/projects/${projectId}/locations/asia-northeast3/services/momso-any-${short}`;
  const resources = new Map([
    [r, { name: `projects/${projectId}/locations/asia-northeast3/services/momso-any-${short}`, labels: { 'momso-install-id': id }, etag: 'fixture' }],
    [`${c}zones/asia-northeast3-a/instances/${name}`, { name, id: 'vm-1', labels: { 'momso-install-id': id }, disks: [{ boot: true, autoDelete: true, source: `/disks/${name}` }], serviceAccounts: [{ email: `momso-vm-${short}@${projectId}.iam.gserviceaccount.com` }] }],
    [`${c}regions/asia-northeast3/routers/${name}-router`, { name: `${name}-router`, description: marker, nats: [{ name: `${name}-nat` }] }],
    [`${c}global/firewalls/${name}-private`, { name: `${name}-private`, description: marker, network: `/networks/${name}-net` }],
    [`${c}regions/asia-northeast3/subnetworks/${name}-sub`, { name: `${name}-sub`, description: marker, network: `/networks/${name}-net` }],
    [`${c}global/networks/${name}-net`, { name: `${name}-net`, description: marker }],
    ...['transport', 'session', 'vault'].map(suffix => [`${s}momso-${suffix}-${short}`, { name: `projects/${projectId}/secrets/momso-${suffix}-${short}`, labels: { 'momso-install-id': id } }]),
    ...['vm', 'gateway'].map(suffix => [`${i}serviceAccounts/momso-${suffix}-${short}@${projectId}.iam.gserviceaccount.com`, { email: `momso-${suffix}-${short}@${projectId}.iam.gserviceaccount.com`, description: marker }]),
    [`${i}roles/momsoStop${short}`, { name: `projects/${projectId}/roles/momsoStop${short}`, description: marker, includedPermissions: ['compute.instances.stop'] }],
    [`${i}roles/momsoAnywhere${short}`, { name: `projects/${projectId}/roles/momsoAnywhere${short}`, description: marker, includedPermissions: ['compute.instances.get', 'compute.instances.start'] }],
    [`${i}roles/momsoUpdate${short}`, { name: `projects/${projectId}/roles/momsoUpdate${short}`, description: marker, includedPermissions: ['run.operations.get', 'run.revisions.get', 'run.revisions.list', 'run.services.get', 'run.services.update'] }],
  ]);
  const deleted = [];
  const policies = new Map();
  let networkHeld = true;
  const provider = new GoogleBrowserProvider('fixture-token', { projectId, sleep: async () => {}, fetchImpl: async (raw, options) => {
    const url = new URL(raw); url.search = '';
    const path = url.href;
    if (path.includes('getIamPolicy') || path.includes('setIamPolicy')) {
      if (denyPolicy && path.includes('getIamPolicy')) return ok({}, 403);
      const key = path.replace(/(?::|\/)(get|set)IamPolicy$/, '');
      if (path.includes('getIamPolicy')) return ok(policies.get(key) ?? { etag: 'fixture', version: 3, bindings: [{ role: 'fixture-role', condition: { title: 'keep' }, members: [`serviceAccount:momso-vm-${short}@${projectId}.iam.gserviceaccount.com`, `deleted:serviceAccount:momso-gateway-${short}@${projectId}.iam.gserviceaccount.com?uid=123`, 'user:other@example.test'] }] });
      policies.set(key, JSON.parse(options.body).policy); return ok({});
    }
    if (path.includes('cloudresourcemanager.googleapis.com')) return ok({ projectId, state: 'ACTIVE' });
    if (path.includes('/operations/')) return path.includes('run.googleapis.com') ? ok({ done: true }) : ok({ status: 'DONE' });
    if (options.method === 'GET') return resources.has(path) ? ok(resources.get(path)) : ok({}, 404);
    if (options.method === 'PATCH' && path.includes('/routers/')) { resources.get(path).nats = []; return ok({ name: 'fixture-op', region: '/regions/asia-northeast3' }); }
    assert.equal(options.method, 'DELETE');
    if (path.includes('/instances/') && rejectVm) return ok({}, 403);
    if (path.includes('/instances/') && loseVmResponse) throw Error('fixture-network');
    if (path.includes('/subnetworks/') && networkHeld) { networkHeld = false; return ok({}, 400); }
    deleted.push(path);
    if (!(retainVm && path.includes('/instances/'))) resources.delete(path);
    if (path.includes('run.googleapis.com')) return ok({ name: `projects/${projectId}/locations/asia-northeast3/operations/fixture` });
    if (path.includes('compute.googleapis.com')) return ok({ name: 'fixture-op', ...(path.includes('/zones/') ? { zone: '/zones/asia-northeast3-a' } : path.includes('/regions/') ? { region: '/regions/asia-northeast3' } : {}) });
    return ok({});
  } });
  return { provider, resources, deleted, policies, router: `${c}regions/asia-northeast3/routers/${name}-router` };
}

test('owned cleanup reports delayed subnet and safely resumes', async () => {
  const f = fixture();
  const first = await f.provider.destroy(id);
  assert(f.policies.size > 0);
  for (const policy of f.policies.values()) {
    assert.deepEqual(policy.bindings[0].members, ['user:other@example.test']);
    assert.equal(policy.bindings[0].condition.title, 'keep');
  }
  assert.match(f.deleted[0], /\/instances\//);
  assert.match(f.deleted.at(-1), /run\.googleapis\.com/);
  assert.deepEqual(first.remaining, ['subnet', 'network']);
  assert.equal(f.resources.size, 6);
  const second = await f.provider.destroy(id);
  assert.deepEqual(second.remaining, []);
  assert.equal(f.resources.size, 4);
  assert([...f.resources.keys()].some(value => value.endsWith(`/secrets/momso-vault-${short}`)));
  assert(!f.deleted.some(value => value.endsWith(`/secrets/momso-vault-${short}`)));
  assert(!f.deleted.some(value => value.includes('cloudresourcemanager.googleapis.com')));
});

test('VM deletion reports definite rejection separately from a lost response', async () => {
  for (const [options, expected] of [[{ rejectVm: true }, ['unknown', 'failed']], [{ loseVmResponse: true }, ['unknown']]]) {
    const f = fixture(options), states = [];
    await assert.rejects(f.provider.destroy(id, () => {}, { onVmState: value => states.push(value) }));
    assert.deepEqual(states, expected);
    assert([...f.resources.keys()].some(value => value.includes('run.googleapis.com')));
  }
});

test('unreadable IAM policy stops before any resource deletion', async () => {
  const f = fixture({ denyPolicy: true });
  await assert.rejects(f.provider.destroy(id), { step: 'iam_bindings_unverified' });
  assert.deepEqual(f.deleted, []);
});

test('unconfirmed VM removal leaves the gateway and all other resources intact', async () => {
  const f = fixture({ retainVm: true });
  await assert.rejects(f.provider.destroy(id), { step: 'vm_deletion_unconfirmed' });
  assert.equal(f.deleted.length, 1);
  assert.match(f.deleted[0], /\/instances\//);
  assert([...f.resources.keys()].some(value => value.includes('run.googleapis.com')));
});

test('foreign resource or additional router attachment aborts before any deletion', async () => {
  for (const unsafe of ['owner', 'peer', 'disk', 'update-role']) {
    const f = fixture();
    if (unsafe === 'owner') f.resources.get(f.router).description = 'foreign';
    else if (unsafe === 'peer') f.resources.get(f.router).bgpPeers = [{ name: 'unrelated' }];
    else if (unsafe === 'disk') [...f.resources.values()].find(value => value.name === name).disks.push({ boot: false, autoDelete: false, source: '/disks/unrelated' });
    else [...f.resources.values()].find(value => value.name?.endsWith(`/roles/momsoUpdate${short}`)).description = 'foreign';
    await assert.rejects(f.provider.destroy(id), { step: 'resource_ownership' });
    assert.deepEqual(f.deleted, []);
  }
});

test('browser rejects resumed deletion before touching resources when backup freshness is unverified', async () => {
  const f = fixture();
  const mem = () => { const values = new Map(); return { getItem: key => values.get(key) ?? null, setItem: (key, value) => values.set(key, value), removeItem: key => values.delete(key), values }; };
  const session = mem(), storage = mem();
  const clientId = 'fixture.apps.googleusercontent.com';
  const location = { href: 'https://login.example.test/start/', origin: 'https://login.example.test', assign(value) { this.next = value; } };
  async function run() {
    clearStartAuthorization();
    return destroyFromGesture({ clientId, projectId, installationId: id, session, location, cryptoImpl: webcrypto });
  }
  assert.equal((await run()).reason, 'pause_requires_app_session');
  beginRedirect({ clientId, action: 'destroy', projectId, installationId: id, storage: session, location, cryptoImpl: webcrypto });
  const state = new URL(location.next).searchParams.get('state');
  const fragment = new URLSearchParams({ state, access_token: 'fixture-token', token_type: 'Bearer', expires_in: '60', scope: 'openid email https://www.googleapis.com/auth/cloud-platform' }).toString();
  assert.equal((await resumeFromRedirect({ clientId, release: {}, session, storage, location, fragment, cryptoImpl: webcrypto, providerFactory: async () => ({ provider: f.provider, ownerSub: '12345' }) })).value.reason, 'pause_requires_app_session');
  assert.deepEqual(f.deleted, []);
  assert(!JSON.stringify([...storage.values]).includes('fixture-token'));
});

test('explicit discard survives authorization and removes only owned fixture resources', async () => {
  const f = fixture();
  const mem = () => { const values = new Map(); return { getItem: key => values.get(key) ?? null, setItem: (key, value) => values.set(key, value), removeItem: key => values.delete(key) }; };
  const session = mem(), storage = mem();
  const clientId = 'fixture.apps.googleusercontent.com';
  const location = { href: 'https://login.example.test/start/', origin: 'https://login.example.test', assign(value) { this.next = value; } };
  clearStartAuthorization();
  await destroyFromGesture({ clientId, projectId, installationId: id, discardData: true, session, location, cryptoImpl: webcrypto });
  const state = new URL(location.next).searchParams.get('state');
  const fragment = new URLSearchParams({ state, access_token: 'fixture-token', token_type: 'Bearer', expires_in: '60', scope: 'openid email https://www.googleapis.com/auth/cloud-platform' }).toString();
  const result = await resumeFromRedirect({ clientId, release: {}, session, storage, location, fragment, cryptoImpl: webcrypto, providerFactory: async () => ({ provider: f.provider, ownerSub: '12345' }) });
  assert.equal(result.value.mode, 'delete_all');
  assert.equal(result.value.status, 'pending');
  assert(f.deleted.length > 0);
  assert(!f.deleted.some(value => value.endsWith(`/secrets/momso-vault-${short}`)));
});

test('PC command rejects invalid project without logging its input token', () => {
  const child = spawnSync(process.execPath, ['scripts/momso_anywhere_destroy_cli.mjs', '--project', 'INVALID', '--installation-id', id, '--state', 'unused-fixture.json'], { input: 'fixture-private-token\n', encoding: 'utf8' });
  assert.equal(child.status, 1);
  assert(!child.stdout.includes('fixture-private-token'));
  assert(!child.stderr.includes('fixture-private-token'));
});

test('PC command refuses before touching cloud resources if backup freshness is unverified', async () => {
  const dir = await mkdtemp(join(tmpdir(), 'momso-destroy-test-'));
  try {
    const f = fixture();
    const path = join(dir, 'state.json');
    const options = { projectId, installationId: id, token: 'fixture-private-token', path, providerFactory: () => f.provider };
    await assert.rejects(executeDestroy(options), { step: 'backup_freshness_unverified' });
    assert.deepEqual(f.deleted, []);
    await assert.rejects(readFile(path, 'utf8'), { code: 'ENOENT' });
    const result = await executeDestroy({ ...options, discardData: true });
    assert.equal(result.mode, 'delete_all');
    assert(f.deleted.length > 0);
  } finally { await rm(dir, { recursive: true, force: true }); }
});
