import assert from 'node:assert/strict';
import test from 'node:test';
import { webcrypto } from 'node:crypto';
import { spawnSync } from 'node:child_process';
import { mkdtemp, readFile, rm, writeFile } from 'node:fs/promises';
import { tmpdir } from 'node:os';
import { join } from 'node:path';
import { GoogleBrowserProvider } from './momso_anywhere_google_browser.mjs';
import { runDestroyCommand } from './momso_anywhere_destroy_command.mjs';
import { executeDestroy } from './momso_anywhere_destroy_cli.mjs';
import { clearStartAuthorization, destroyFromGesture, resumeFromRedirect } from './momso_anywhere_start.mjs';

const id = 'a'.repeat(24), short = id.slice(0, 12), projectId = 'fixture-project', name = `momso-${short}`;
const marker = `momso-install-${id}`;
const ok = (value, status = 200) => ({ ok: status >= 200 && status < 300, status, json: async () => value });

function fixture() {
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
  let networkHeld = true;
  const provider = new GoogleBrowserProvider('fixture-token', { projectId, sleep: async () => {}, fetchImpl: async (raw, options) => {
    const url = new URL(raw); url.search = '';
    const path = url.href;
    if (path.includes('cloudresourcemanager.googleapis.com')) return ok({ projectId, state: 'ACTIVE' });
    if (path.includes('/operations/')) return path.includes('run.googleapis.com') ? ok({ done: true }) : ok({ status: 'DONE' });
    if (options.method === 'GET') return resources.has(path) ? ok(resources.get(path)) : ok({}, 404);
    if (options.method === 'PATCH' && path.includes('/routers/')) { resources.get(path).nats = []; return ok({ name: 'fixture-op', region: '/regions/asia-northeast3' }); }
    assert.equal(options.method, 'DELETE');
    if (path.includes('/subnetworks/') && networkHeld) { networkHeld = false; return ok({}, 400); }
    deleted.push(path); resources.delete(path);
    if (path.includes('run.googleapis.com')) return ok({ name: `projects/${projectId}/locations/asia-northeast3/operations/fixture` });
    if (path.includes('compute.googleapis.com')) return ok({ name: 'fixture-op', ...(path.includes('/zones/') ? { zone: '/zones/asia-northeast3-a' } : path.includes('/regions/') ? { region: '/regions/asia-northeast3' } : {}) });
    return ok({});
  } });
  return { provider, resources, deleted, router: `${c}regions/asia-northeast3/routers/${name}-router` };
}

test('owned cleanup records delayed subnet and safely resumes', async () => {
  const f = fixture();
  let recorded;
  const first = await runDestroyCommand({ projectId, installationId: id, token: 'fixture-token', providerFactory: () => f.provider, saveState: async value => { recorded = value; } });
  assert.deepEqual(first.remaining, ['subnet', 'network']);
  assert.deepEqual(recorded, { projectId, installationId: id, remaining: ['subnet', 'network'] });
  assert(!JSON.stringify(recorded).includes('fixture-token'));
  assert.equal(f.resources.size, 2);
  const second = await f.provider.destroy(id);
  assert.deepEqual(second.remaining, []);
  assert.equal(f.resources.size, 0);
  assert(!f.deleted.some(value => value.includes('cloudresourcemanager.googleapis.com')));
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

test('browser records remaining resources and resumes after reauthorization', async () => {
  const f = fixture();
  const mem = () => { const values = new Map(); return { getItem: key => values.get(key) ?? null, setItem: (key, value) => values.set(key, value), removeItem: key => values.delete(key), values }; };
  const session = mem(), storage = mem();
  const clientId = 'fixture.apps.googleusercontent.com';
  const location = { href: 'https://login.example.test/start/', origin: 'https://login.example.test', assign(value) { this.next = value; } };
  async function run() {
    clearStartAuthorization();
    await destroyFromGesture({ clientId, projectId, installationId: id, session, location, cryptoImpl: webcrypto });
    const state = new URL(location.next).searchParams.get('state');
    const fragment = new URLSearchParams({ state, access_token: 'fixture-token', token_type: 'Bearer', expires_in: '60', scope: 'openid email https://www.googleapis.com/auth/cloud-platform' }).toString();
    return resumeFromRedirect({ clientId, release: {}, session, storage, location, fragment, cryptoImpl: webcrypto, providerFactory: async () => ({ provider: f.provider, ownerSub: '12345' }) });
  }
  const first = await run();
  assert.deepEqual(first.value.remaining, ['subnet', 'network']);
  assert(!JSON.stringify([...storage.values]).includes('fixture-token'));
  assert.deepEqual((await run()).value.remaining, []);
  assert.equal(storage.values.size, 0);
});

test('PC command rejects invalid project without logging its input token', () => {
  const child = spawnSync(process.execPath, ['scripts/momso_anywhere_destroy_cli.mjs', '--project', 'INVALID', '--installation-id', id, '--state', 'unused-fixture.json'], { input: 'fixture-private-token\n', encoding: 'utf8' });
  assert.equal(child.status, 1);
  assert(!child.stdout.includes('fixture-private-token'));
  assert(!child.stderr.includes('fixture-private-token'));
});

test('PC command saves delayed cleanup and resumes without accepting another installation state', async () => {
  const dir = await mkdtemp(join(tmpdir(), 'momso-destroy-test-'));
  try {
    const f = fixture();
    const path = join(dir, 'state.json');
    const options = { projectId, installationId: id, token: 'fixture-private-token', path, providerFactory: () => f.provider };
    assert.deepEqual((await executeDestroy(options)).remaining, ['subnet', 'network']);
    const saved = await readFile(path, 'utf8');
    assert(!saved.includes('fixture-private-token'));
    assert.deepEqual(JSON.parse(saved).remaining, ['subnet', 'network']);
    await writeFile(path, JSON.stringify({ projectId, installationId: 'b'.repeat(24) }));
    await assert.rejects(executeDestroy(options), /state_unavailable/);
    assert.equal(f.resources.size, 2);
    await writeFile(path, saved);
    assert.deepEqual((await executeDestroy(options)).remaining, []);
    assert.deepEqual(JSON.parse(await readFile(path, 'utf8')).remaining, []);
  } finally { await rm(dir, { recursive: true, force: true }); }
});
