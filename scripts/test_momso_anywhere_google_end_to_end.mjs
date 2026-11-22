import assert from 'node:assert/strict';
import { webcrypto } from 'node:crypto';
import { spawnSync } from 'node:child_process';
import test from 'node:test';
import { GoogleBrowserProvider } from './momso_anywhere_google_browser.mjs';
import { startFromGesture } from './momso_anywhere_start.mjs';
import { signedFixture } from './test_momso_release_fixture.mjs';
import { verifyRelease } from './momso_release_manifest.mjs';

const id = 'a'.repeat(24);
const short = id.slice(0, 12);
const ok = (body = {}, status = 200) => ({ ok: status >= 200 && status < 300, status, json: async () => body });

for (const [existing, channel] of [[false, 'production'], [true, 'production'], [false, 'development']]) test(`start screen resumes a whole fake Google REST installation after interruption (${existing ? 'existing' : 'new'} project, ${channel})`, async () => {
  const projectId = existing ? 'fixture-project' : `momso-${id}`;
  const resources = new Map();
  const accounts = new Map();
  const secrets = new Map();
  const roles = new Map();
  const secretPolicies = new Map();
  const backupResources = new Map();
  const backupPolicies = new Map();
  let project = existing ? { projectId, state: 'ACTIVE', labels: { 'unrelated-label': 'preserve' } } : null;
  let billing = existing;
  let vmFailed = false;
  let service = null;
  const writes = [];
  const signed = await signedFixture({ channel });
  const release = { ...signed.release, channel };
  const fetchImpl = async (raw, options) => {
    const url = new URL(raw);
    const path = url.pathname;
    const method = options.method;
    const body = options.body ? JSON.parse(options.body) : {};
    if (url.hostname === 'fixture.test') return signed.response(path.endsWith('.sig') ? signed.signature : signed.raw);
    if (method !== 'GET') writes.push(`${method} ${url.hostname}${path}`);
    if (url.hostname === 'cloudresourcemanager.googleapis.com') {
      if (path === '/v3/projects' && method === 'POST') { project = { ...body, state: 'ACTIVE' }; return ok({ name: 'operations/create' }); }
      if (path === '/v3/operations/create') return ok({ done: true });
      if (path === `/v3/projects/${projectId}`) return project ? ok(project) : ok({}, 404);
    }
    if (url.hostname === 'cloudbilling.googleapis.com') {
      if (path.endsWith('/billingInfo')) { if (method === 'PUT') billing = true; return ok({ billingEnabled: billing }); }
      if (path === '/v1/billingAccounts') return ok({ billingAccounts: [{ name: 'billingAccounts/FAKE', open: true }] });
    }
    if (url.hostname === 'serviceusage.googleapis.com') {
      if (path.endsWith('/services:batchEnable')) {
        assert(!body.serviceIds.includes('cloudbuild.googleapis.com'));
        assert(!body.serviceIds.includes('artifactregistry.googleapis.com'));
        return ok({ name: 'operations/enable' });
      }
      if (path === '/v1/operations/enable') return ok({ done: true });
    }
    if (url.hostname === 'iam.googleapis.com') {
      if (path.includes('/serviceAccounts/') && path.endsWith(':getIamPolicy')) return ok({ etag: 'fixture', bindings: [] });
      if (path.includes('/serviceAccounts/') && path.endsWith(':setIamPolicy')) return ok(body.policy);
      if (path.endsWith('/serviceAccounts') && method === 'POST') {
        const email = `${body.accountId}@${projectId}.iam.gserviceaccount.com`;
        accounts.set(email, { email, description: body.serviceAccount.description }); return ok({});
      }
      if (path.includes('/serviceAccounts/')) { const name = path.split('/').at(-1); if (method === 'DELETE') { accounts.delete(name); return ok({}); } return accounts.has(name) ? ok(accounts.get(name)) : ok({}, 404); }
      if (path.endsWith('/roles') && method === 'POST') { roles.set(body.roleId, { ...body.role, name: `projects/${projectId}/roles/${body.roleId}` }); return ok(body.role); }
      if (path.includes('/roles/')) { const name = path.split('/').at(-1); if (method === 'DELETE') { roles.delete(name); return ok({}); } return roles.has(name) ? ok(roles.get(name)) : ok({}, 404); }
    }
    if (url.hostname === 'secretmanager.googleapis.com') {
      if (path.endsWith('/secrets') && method === 'POST') { const name = url.searchParams.get('secretId'); secrets.set(name, { name: `projects/${projectId}/secrets/${name}`, labels: { 'momso-install-id': id }, versions: [] }); return ok({}); }
      const name = path.split('/secrets/')[1]?.split(/[/:]/)[0];
      if (method === 'DELETE') { secrets.delete(name); return ok({}); }
      if (path.endsWith(':getIamPolicy')) return ok(secretPolicies.get(name) ?? { etag: 'fixture', bindings: [] });
      if (path.endsWith(':setIamPolicy')) { secretPolicies.set(name, body.policy); return ok({}); }
      if (path.endsWith(':addVersion')) { secrets.get(name).versions.push({ state: 'ENABLED' }); return ok({}); }
      if (path.endsWith('/versions/1')) return secrets.get(name)?.versions?.[0] ? ok(secrets.get(name).versions[0]) : ok({}, 404);
      if (path.endsWith('/versions')) return ok({ versions: secrets.get(name).versions });
      return secrets.has(name) ? ok(secrets.get(name)) : ok({}, 404);
    }
    if (url.hostname === 'storage.googleapis.com' || url.hostname === 'cloudkms.googleapis.com') {
      const bucketPath = `/storage/v1/b/momso-backup-${short}`;
      const ringPath = `/v1/projects/${projectId}/locations/asia-northeast3/keyRings/momso-backup-${short}`;
      const keyPath = `${ringPath}/cryptoKeys/backup`;
      if (path.endsWith('/iam') || path.endsWith(':getIamPolicy') || path.endsWith(':setIamPolicy')) {
        const key = path.replace(/\/iam$|:getIamPolicy$|:setIamPolicy$/, '');
        if (method !== 'GET') { backupPolicies.set(key, body.policy ?? body); return ok({}); }
        return ok(backupPolicies.get(key) ?? { etag: 'fixture', bindings: [] });
      }
      if (method === 'POST' && path === '/storage/v1/b') { backupResources.set(bucketPath, body); return ok(body); }
      if (method === 'POST' && path.endsWith('/keyRings')) { const value = { name: ringPath.slice(4) }; backupResources.set(ringPath, value); return ok(value); }
      if (method === 'POST' && path.endsWith('/cryptoKeys')) { const value = { ...body, name: keyPath.slice(4) }; backupResources.set(keyPath, value); return ok(value); }
      return backupResources.has(path) ? ok(backupResources.get(path)) : ok({}, 404);
    }
    if (url.hostname === 'compute.googleapis.com') {
      if (path.includes('/operations/')) return ok({ status: 'DONE' });
      if (path.endsWith('/getGuestAttributes')) { const vault = secrets.get(`momso-vault-${short}`); if (vault && !vault.versions.length) vault.versions.push({ state: 'ENABLED' }); return ok({ queryValue: { items: [{ namespace: 'momso', key: 'fingerprint', value: 'b'.repeat(64) }] } }); }
      if (path.endsWith('/getIamPolicy')) return ok({ etag: 'fixture', bindings: [] });
      if (path.endsWith('/setIamPolicy')) return ok({});
      const prefix = `/compute/v1/projects/${projectId}/`;
      const key = path.slice(prefix.length);
      if (method === 'GET') return resources.has(key) ? ok(resources.get(key)) : ok({}, 404);
      if (method === 'PATCH' && key.includes('/routers/')) { resources.get(key).nats = []; return ok({ name: 'fixture-op', region: '/regions/asia-northeast3' }); }
      if (method === 'DELETE') { resources.delete(key); return ok({ name: 'fixture-op', ...(key.startsWith('zones/') ? { zone: '/zones/asia-northeast3-a' } : key.startsWith('regions/') ? { region: '/regions/asia-northeast3' } : {}) }); }
      if (method === 'POST') {
        if (key.endsWith('/instances') && !vmFailed) { vmFailed = true; return ok({}, 503); }
        resources.set(`${key}/${body.name}`, { ...body, ...(key.endsWith('/instances') ? { status: 'RUNNING', disks: body.disks.map(disk => ({ ...disk, source: `/disks/${body.name}` })) } : {}) });
        return ok({ name: 'fixture-op', ...(key.startsWith('zones/') ? { zone: '/zones/asia-northeast3-a' } : key.startsWith('regions/') ? { region: '/regions/asia-northeast3' } : {}) });
      }
    }
    if (url.hostname === 'run.googleapis.com') {
      if (path.includes('/operations/')) return ok({ done: true });
      if (path.includes('/revisions/')) return ok({ containers: [{ image: service.template.containers[0].image }] });
      if (path.endsWith(':getIamPolicy')) return ok({ etag: 'fixture', bindings: [] });
      if (path.endsWith(':setIamPolicy')) return ok(body.policy);
      if (method === 'POST') { service = { ...body, name: `projects/${projectId}/locations/asia-northeast3/services/momso-any-${short}`, uri: 'https://fixture.a.run.app', etag: 'fixture', terminalCondition: { state: 'CONDITION_SUCCEEDED' }, latestReadyRevision: 'fixture-rev-1', latestCreatedRevision: 'fixture-rev-1', trafficStatuses: [{ revision: 'fixture-rev-1', percent: 100 }] }; return ok({ name: `projects/${projectId}/locations/asia-northeast3/operations/create` }); }
      if (method === 'DELETE') { service = null; return ok({ name: `projects/${projectId}/locations/asia-northeast3/operations/delete` }); }
      if (method === 'PATCH') { service = { ...service, ...body, ...(body.template ? { latestReadyRevision: 'fixture-rev-2', latestCreatedRevision: 'fixture-rev-2' } : {}), ...(body.traffic ? { trafficStatuses: body.traffic } : {}) }; return ok({ name: `projects/${projectId}/locations/asia-northeast3/operations/update` }); }
      return service ? ok(service) : ok({}, 404);
    }
    assert.fail(`unexpected fake request ${method} ${url.hostname}${path}`);
  };
  const provider = new GoogleBrowserProvider('fixture-private-token', { fetchImpl, sleep: async () => {}, release, clientId: 'fixture.apps.googleusercontent.com', ownerSub: '12345', projectId: existing ? projectId : null });
  const storage = { getItem: () => id, setItem: () => assert.fail('existing checkpoint') };
  const progress = [];
  const args = { clientId: 'fixture.apps.googleusercontent.com', release, projectId: existing ? projectId : null, storage, cryptoImpl: webcrypto, onProgress: value => progress.push(value), authorize: async options => { assert.equal(options.projectId, existing ? projectId : null); return { provider, ownerSub: '12345' }; } };
  await assert.rejects(startFromGesture(args), { step: 'google_unavailable' });
  assert(!secretPolicies.get(`momso-vault-${short}`).bindings.some(item => item.role === 'roles/secretmanager.secretVersionAdder'));
  const result = await startFromGesture(args);
  assert.equal(result.url, 'https://fixture.a.run.app');
  assert.equal(writes.filter(item => item === 'POST cloudresourcemanager.googleapis.com/v3/projects').length, existing ? 0 : 1);
  if (existing) assert.deepEqual(project.labels, { 'unrelated-label': 'preserve' });
  assert.equal(writes.filter(item => item.endsWith('/instances') && item.startsWith('POST')).length, 2);
  assert(progress.some(item => item.step === 'server' && item.state === 'failed'));
  assert(progress.some(item => item.step === 'gatewayReady' && item.state === 'done'));
  assert(!JSON.stringify(writes).includes('fixture-private-token'));
  assert.equal(secrets.size, 3);
  assert(!writes.some(item => item.includes(`momso-vault-${short}:addVersion`)));
  assert(!secretPolicies.get(`momso-vault-${short}`).bindings.some(item => item.role === 'roles/secretmanager.secretVersionAdder'));
  assert.deepEqual(roles.get(`momsoStop${short}`).includedPermissions, ['compute.instances.stop']);
  const vaultAccess = secretPolicies.get(`momso-vault-${short}`).bindings.find(item => item.role === 'roles/secretmanager.secretAccessor');
  assert.deepEqual(vaultAccess.members, [`serviceAccount:momso-vm-${short}@${projectId}.iam.gserviceaccount.com`]);
  assert(resources.has(`zones/asia-northeast3-a/instances/momso-${short}`));
  if (!existing && channel === 'production' && process.env.MOMSO_S68_BOOTSTRAP_OUT) {
    for (const pattern of ['test_momso_anywhere_bootstrap_s68.py', 'test_momso_anywhere_fullstack_s68.py']) {
      const child = spawnSync('python3', ['-m', 'unittest', 'discover', '-s', 'scripts', '-p', pattern], { encoding: 'utf8', env: process.env, timeout: 180000 });
      assert.equal(child.status, 0, `${pattern}: ${child.stdout}\n${child.stderr}`);
    }
  }
  const removal = new GoogleBrowserProvider('fixture-private-token', { fetchImpl, sleep: async () => {}, projectId });
  assert.deepEqual(await removal.destroy(id), { remaining: [] });
  assert.equal(resources.size, 0);
  assert.equal(secrets.size, 0);
  assert.equal(accounts.size, 0);
  assert.equal(roles.size, 0);
  assert.equal(service, null);
  assert(project, 'destroy must preserve the project');
});

test('invalid signed release cannot start a cloud installation', async () => {
  const signed = await signedFixture();
  signed.signature[0] ^= 1;
  let cloudWrites = 0;
  const provider = new GoogleBrowserProvider('fixture-token', { release: signed.release, clientId: 'fixture.apps.googleusercontent.com', ownerSub: '12345', fetchImpl: async url => {
    if (new URL(url).hostname === 'fixture.test') return signed.response(url.endsWith('.sig') ? signed.signature : signed.raw);
    cloudWrites++;
    throw new Error('unexpected cloud access');
  } });
  await assert.rejects(startFromGesture({ clientId: 'fixture.apps.googleusercontent.com', release: signed.release, storage: { getItem: () => id, setItem: () => {} }, cryptoImpl: webcrypto, authorize: async () => ({ provider, ownerSub: '12345' }) }), { step: 'release_not_ready' });
  assert.equal(cloudWrites, 0);
});

test('development and production signed releases do not cross trust boundaries', async () => {
  const dev = await signedFixture({ channel: 'development' });
  const prod = await signedFixture({ channel: 'production' });
  const fetchDev = async url => dev.response(url.endsWith('.sig') ? dev.signature : dev.raw);
  const fetchProd = async url => prod.response(url.endsWith('.sig') ? prod.signature : prod.raw);
  assert.equal((await verifyRelease({ ...dev.release, channel: 'development' }, { fetchImpl: fetchDev, cryptoImpl: webcrypto })).channel, 'development');
  await assert.rejects(verifyRelease({ ...dev.release, channel: 'development' }, { fetchImpl: fetchProd, cryptoImpl: webcrypto }), { step: 'release_not_ready' });
  await assert.rejects(verifyRelease({ ...dev.release, channel: 'production' }, { fetchImpl: fetchDev, cryptoImpl: webcrypto }), { step: 'release_not_ready' });
});
