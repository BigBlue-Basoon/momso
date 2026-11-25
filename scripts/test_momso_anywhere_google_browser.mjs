import assert from 'node:assert/strict';
import test from 'node:test';
import { providerFromToken, GoogleBrowserProvider } from './momso_anywhere_google_browser.mjs';
import { startAnywhere } from './momso_anywhere_setup.mjs';
import { fakeBackupCloud } from './fixture_fake_backup_cloud.mjs';

const id = 'a'.repeat(24);
const projectId = `momso-${id}`;
const response = (data, status = 200) => ({ ok: status >= 200 && status < 300, status, json: async () => data });

test('Google identity keeps credentials in provider memory', async () => {
  const result = await providerFromToken({ token: 'fixture-private-token', expiresAt: Date.now() + 30000, clientId: 'fixture.apps.googleusercontent.com', fetchImpl: async (url, options) => {
    assert.equal(url, 'https://openidconnect.googleapis.com/v1/userinfo');
    assert.equal(options.headers.Authorization, 'Bearer fixture-private-token');
    return response({ sub: '12345' });
  } });
  assert.equal(result.ownerSub, '12345');
  assert(!JSON.stringify(result).includes('fixture-private-token'));
});

test('Google project creation resumes only an owned project', async () => {
  let existing = null;
  let creations = 0;
  const calls = [];
  const provider = new GoogleBrowserProvider('fixture-secret', { sleep: async () => {}, fetchImpl: async (url, options) => {
    calls.push({ url, method: options.method });
    assert.equal(options.headers.Authorization, 'Bearer fixture-secret');
    if (url.endsWith(`/projects/${projectId}`)) return existing ? response(existing) : response({}, 404);
    if (url.endsWith('/v3/projects') && options.method === 'POST') {
      creations++;
      existing = { projectId, state: 'ACTIVE', labels: { 'momso-install-id': id } };
      return response({ name: 'operations/fixture' });
    }
    if (url.endsWith('/operations/fixture')) return response({ done: true });
    assert.fail(url);
  } });
  await provider.project(id);
  await provider.project(id);
  assert.equal(creations, 1);
  existing.labels = { 'momso-install-id': 'other' };
  await assert.rejects(provider.project(id), { step: 'project_ownership' });
  assert(!JSON.stringify(provider).includes('fixture-secret'));
  assert(!JSON.stringify(calls).includes('fixture-secret'));
});

test('selected existing project is read only and is not silently recreated', async () => {
  let exists = true;
  const methods = [];
  const provider = new GoogleBrowserProvider('fixture', { projectId: 'fixture-project', fetchImpl: async (_url, options) => {
    methods.push(options.method);
    return exists ? response({ projectId: 'fixture-project', state: 'ACTIVE', labels: { other: 'keep' } }) : response({}, 404);
  } });
  await provider.project(id);
  exists = false;
  await assert.rejects(provider.project(id), { step: 'project_unavailable' });
  assert.deepEqual(methods, ['GET', 'GET']);
});

test('billing never silently changes an existing account or picks between several', async () => {
  let billing = { billingEnabled: false, billingAccountName: 'billingAccounts/EXISTING' };
  let writes = 0;
  const provider = new GoogleBrowserProvider('fixture', { fetchImpl: async (url, options) => {
    if (options.method === 'PUT') writes++;
    if (url.endsWith('/billingInfo')) return response(billing);
    return response({ billingAccounts: [{ name: 'billingAccounts/ONE', open: true }, { name: 'billingAccounts/TWO', open: true }] });
  } });
  await assert.rejects(provider.billing(id), { step: 'billing_inactive' });
  billing = { billingEnabled: false };
  await assert.rejects(provider.billing(id), { step: 'billing_selection_required' });
  assert.equal(writes, 0);
});

test('no accessible open billing account has a distinct reason without creating one', async () => {
  const methods = [];
  const provider = new GoogleBrowserProvider('fixture', { fetchImpl: async (url, options) => {
    methods.push(options.method);
    return url.endsWith('/billingInfo') ? response({ billingEnabled: false }) : response({ billingAccounts: [] });
  } });
  await assert.rejects(provider.billing(id), { step: 'billing_account_missing' });
  assert.deepEqual(methods, ['GET', 'GET']);
});

test('a 403 is not misrepresented as terms or card failure', async () => {
  const provider = new GoogleBrowserProvider('fixture', { fetchImpl: async () => response({ error: { message: 'fixture' } }, 403) });
  await assert.rejects(provider.billingAccounts(), { step: 'permission_denied', checks: ['billing_account', 'terms', 'project_permissions'] });
});

test('billing lists pages and connects only the selected open account', async () => {
  let enabled = false;
  const writes = [];
  const provider = new GoogleBrowserProvider('fixture', { billingAccountName: 'billingAccounts/TWO', fetchImpl: async (url, options) => {
    if (url.includes('/billingInfo')) {
      if (options.method === 'PUT') { writes.push(JSON.parse(options.body)); enabled = true; }
      return response({ billingEnabled: enabled });
    }
    const page = new URL(url).searchParams.get('pageToken');
    return page ? response({ billingAccounts: [{ name: 'billingAccounts/TWO', displayName: 'Second', open: true }, { name: 'billingAccounts/CLOSED', open: false }] }) : response({ billingAccounts: [{ name: 'billingAccounts/ONE', displayName: 'First', open: true }], nextPageToken: 'next' });
  } });
  assert.deepEqual(await provider.billingAccounts(), [{ name: 'billingAccounts/ONE', displayName: 'First' }, { name: 'billingAccounts/TWO', displayName: 'Second' }]);
  await provider.billing(id);
  assert.deepEqual(writes, [{ billingAccountName: 'billingAccounts/TWO' }]);
});

test('incomplete Google provider cannot create partial billable resources through the product flow', async () => {
  let requests = 0;
  const provider = new GoogleBrowserProvider('fixture', { fetchImpl: async () => { requests++; return response({}); } });
  await assert.rejects(startAnywhere(provider, { installationId: id }), { step: 'release_not_ready' });
  assert.equal(requests, 0);
});

test('browser preflight accepts the default Cloud Run static login origin', async () => {
  const release = {
    bundleUrl: 'https://example.test/release.tar.gz', bundleSha256: 'f'.repeat(64),
    gatewayImage: `registry.example.test/team/gateway@sha256:${'c'.repeat(64)}`,
    loginOrigin: 'https://static-123.asia-northeast3.run.app',
  };
  const provider = new GoogleBrowserProvider('fixture', { release, releaseVerifier: async value => value, clientId: 'fixture.apps.googleusercontent.com', ownerSub: '12345', fetchImpl: async () => assert.fail('no cloud calls') });
  await provider.preflight();
});

test('private network is reconciled and a widened firewall is rejected', async () => {
  const resources = new Map();
  let creates = 0;
  const provider = new GoogleBrowserProvider('fixture', { sleep: async () => {}, fetchImpl: async (url, options) => {
    if (url.includes('/operations/')) return response({ status: 'DONE' });
    const base = `https://compute.googleapis.com/compute/v1/projects/${projectId}/`;
    assert(url.startsWith(base));
    const path = url.slice(base.length);
    if (options.method === 'GET') return resources.has(path) ? response(resources.get(path)) : response({}, 404);
    const body = JSON.parse(options.body);
    const key = `${path}/${body.name}`;
    resources.set(key, body);
    creates++;
    return response({ name: 'fixture-op', ...(path.startsWith('regions/') ? { region: 'https://compute.googleapis.com/compute/v1/projects/fixture/regions/asia-northeast3' } : {}) });
  } });
  await provider.privateNetwork(id);
  await provider.privateNetwork(id);
  assert.equal(creates, 4);
  const firewall = [...resources.entries()].find(([key]) => key.includes('/firewalls/'))[1];
  firewall.sourceRanges = ['0.0.0.0/0'];
  await assert.rejects(provider.privateNetwork(id), { step: 'resource_ownership' });
});

test('private identities use per-secret grants and preserve existing IAM conditions', async () => {
  const accounts = new Map();
  const secrets = new Map();
  const policies = new Map();
  const provider = new GoogleBrowserProvider('fixture', { fetchImpl: async (url, options) => {
    if (url.endsWith(`/v3/projects/${projectId}`)) return response({ projectId, state: 'ACTIVE', labels: { 'momso-install-id': id } });
    if (url.includes('iam.googleapis.com')) {
      if (options.method === 'POST') {
        const body = JSON.parse(options.body);
        const email = `${body.accountId}@${projectId}.iam.gserviceaccount.com`;
        accounts.set(email, { email, description: body.serviceAccount.description });
        return response(accounts.get(email));
      }
      const email = url.split('/').at(-1);
      return accounts.has(email) ? response(accounts.get(email)) : response({}, 404);
    }
    if (url.includes('secretmanager.googleapis.com')) {
      const path = url.split('/secrets/')[1]?.split('?')[0];
      if (options.method === 'POST' && url.includes('?secretId=')) {
        const name = url.split('?secretId=')[1];
        secrets.set(name, { labels: { 'momso-install-id': id }, versions: [] });
        return response({});
      }
      const name = path?.split(/[/:]/)[0];
      if (url.includes(':getIamPolicy')) return response(policies.get(name) ?? { etag: 'fixture', bindings: [{ role: 'roles/reader', members: ['user:fixture@example.test'], condition: { title: 'keep' } }] });
      if (url.includes(':setIamPolicy')) { policies.set(name, JSON.parse(options.body).policy); return response({}); }
      if (url.includes(':addVersion')) { secrets.get(name).versions.push({ state: 'ENABLED' }); return response({}); }
      if (url.endsWith('/versions')) return response({ versions: secrets.get(name).versions });
      return secrets.has(name) ? response(secrets.get(name)) : response({}, 404);
    }
    assert.fail(url);
  } });
  await provider.preparePrivateIdentity(id);
  await provider.preparePrivateIdentity(id);
  assert.equal(accounts.size, 2);
  assert.equal(secrets.size, 3);
  for (const [name, secret] of secrets) {
    assert.equal(secret.versions.length, name.includes('vault') ? 0 : 1);
    const policy = policies.get(name);
    assert.equal(policy.version, 3);
    assert.equal(policy.bindings[0].condition.title, 'keep');
    const members = policy.bindings.find(binding => binding.role === 'roles/secretmanager.secretAccessor').members;
    assert.equal(members.length, name.includes('transport') ? 2 : 1);
    if (name.includes('vault')) {
      const temporary = policy.bindings.find(binding => binding.role === 'roles/secretmanager.secretVersionAdder');
      assert.equal(temporary.members.length, 1);
      assert.match(temporary.condition.expression, /^request\.time < timestamp\("[^"]+"\)$/);
    }
  }
});

test('VM is private, verifies its release, and retries the same instance', async () => {
  let instance = null;
  let creates = 0;
  let identities = 0;
  let role = null;
  const backup = fakeBackupCloud(projectId, id, response);
  const release = { bundleUrl: 'https://example.test/momso-release.tar.gz', bundleSha256: 'f'.repeat(64), gatewayImage: `registry.example.test/team/gateway@sha256:${'c'.repeat(64)}`, loginOrigin: 'https://login.example.test' };
  const provider = new GoogleBrowserProvider('fixture', { release, releaseVerifier: async value => value, clientId: 'fixture.apps.googleusercontent.com', ownerSub: '12345', sleep: async () => {}, fetchImpl: async (url, options) => {
    const backupResponse = backup(url, options);
    if (backupResponse) return backupResponse;
    if (url.includes('iam.googleapis.com') && url.includes('/roles')) {
      if (options.method === 'POST') { role = JSON.parse(options.body).role; return response(role); }
      return role ? response(role) : response({}, 404);
    }
    if (url.includes('/getIamPolicy')) return response({ etag: 'fixture', bindings: [{ role: 'roles/reader', members: ['user:fixture@example.test'] }] });
    if (url.includes('/setIamPolicy')) return response({});
    if (url.includes('secretmanager.googleapis.com')) return response(url.includes(':getIamPolicy') ? { etag: 'fixture', bindings: [] } : url.endsWith('/versions/1') ? { state: 'ENABLED' } : {});
    if (url.includes('/operations/')) return response({ status: 'DONE' });
    if (url.includes('/getGuestAttributes')) return response({ queryValue: { items: [{ namespace: 'momso', key: 'fingerprint', value: 'b'.repeat(64) }] } });
    if (options.method === 'POST') {
      instance = { ...JSON.parse(options.body), status: 'RUNNING' };
      creates++;
      return response({ name: 'fixture-op', zone: 'https://compute.googleapis.com/compute/v1/projects/fixture/zones/asia-northeast3-a' });
    }
    return instance ? response(instance) : response({}, 404);
  } });
  provider.preparePrivateIdentity = async () => { identities++; };
  await provider.server(id);
  await provider.server(id);
  assert.equal(creates, 1);
  assert.equal(identities, 2);
  assert.equal(instance.networkInterfaces[0].networkIP, '10.42.0.10');
  assert.equal(instance.networkInterfaces[0].accessConfigs, undefined);
  assert(instance.metadata.items.find(item => item.key === 'startup-script').value.includes(release.bundleSha256));
  instance.networkInterfaces[0].accessConfigs = [{ natIP: '203.0.113.1' }];
  await assert.rejects(provider.server(id), { step: 'resource_ownership' });
});

test('gateway provisions pinned image and secrets privately before opening its authenticated entry', async () => {
  const release = { bundleUrl: 'https://example.test/release.tar.gz', bundleSha256: 'f'.repeat(64), gatewayImage: `registry.example.test/team/gateway@sha256:${'c'.repeat(64)}`, loginOrigin: 'https://login.example.test' };
  const roles = new Map();
  let service = null;
  let vmPolicy = { etag: 'fixture', bindings: [{ role: 'roles/reader', members: ['user:fixture@example.test'], condition: { title: 'keep' } }] };
  let publicBindings = false;
  let bindingAttempts = 0;
  let proxyReady = true;
  const actions = [];
  const updatePolicies = [];
  const provider = new GoogleBrowserProvider('fixture', { release, releaseVerifier: async value => value, clientId: 'fixture.apps.googleusercontent.com', ownerSub: '12345', sleep: async () => {}, fetchImpl: async (url, options) => {
    if (url === 'https://fixture.a.run.app/__momso/ready') return response({ ready: true });
    if (url === 'https://fixture.a.run.app/__momso/proxy-health') {
      assert.equal(options.credentials, 'omit');
      assert.match(options.headers['X-Momso-Health-Proof'], /^[a-f0-9]{64}$/);
      return response(proxyReady ? { state: 'ready' } : { error: 'unavailable' }, proxyReady ? 200 : 503);
    }
    if (url.includes('secretmanager.googleapis.com') && url.endsWith('/versions/latest:access')) return response({ payload: { data: Buffer.from('fixture-private-transport-token-value-1234567890').toString('base64') } });
    if (url.includes('secretmanager.googleapis.com') && url.includes('/secrets/momso-transport-')) return response({ labels: { 'momso-install-id': id } });
    const method = options.method;
    if (url.includes('/getGuestAttributes')) return response({ queryValue: { items: [{ namespace: 'momso', key: 'fingerprint', value: 'b'.repeat(64) }] } });
    if (url.includes('iam.googleapis.com') && url.includes('/roles')) {
      if (method === 'POST') { const body = JSON.parse(options.body); roles.set(body.roleId, body.role); return response(body.role); }
      const role = roles.get(url.split('/').pop());
      return role ? response(role) : response({}, 404);
    }
    if (url.includes('/getIamPolicy') && url.includes('compute.googleapis.com')) return response(vmPolicy);
    if (url.includes('/setIamPolicy') && url.includes('compute.googleapis.com')) { if (++bindingAttempts < 3) return response({ error: { message: 'Role (projects/fixture-project/roles/momsoFixture) does not exist in the resource\'s hierarchy.' } }, 400); vmPolicy = JSON.parse(options.body).policy; return response(vmPolicy); }
    if (url.includes('/operations/')) return response({ done: true });
    if (url.endsWith(':getIamPolicy?options.requestedPolicyVersion=3')) return response({ etag: 'fixture', bindings: publicBindings ? [{ role: 'roles/run.invoker', members: ['allUsers'] }] : [] });
    if (url.endsWith(':setIamPolicy')) { updatePolicies.push({ url, policy: JSON.parse(options.body).policy }); return response({}); }
    if (url.includes('run.googleapis.com')) {
      if (url.includes('/revisions/')) return response({ conditions: [{ type: 'Ready', state: 'CONDITION_SUCCEEDED', revisionReason: 'RETIRED' }], containers: [{ image: service.template.containers[0].image }] });
      if (method === 'POST') {
        const body = JSON.parse(options.body);
        assert.equal(body.traffic, undefined, 'first create must use Cloud Run default traffic');
        actions.push({ type: 'create', value: body.invokerIamDisabled });
        service = { ...body, name: `projects/${projectId}/locations/asia-northeast3/services/momso-any-${id.slice(0, 12)}`, uri: 'https://fixture.a.run.app', etag: 'fixture', terminalCondition: { state: 'CONDITION_SUCCEEDED' }, latestReadyRevision: 'fixture-rev-1', latestCreatedRevision: 'fixture-rev-1', trafficStatuses: [{ revision: 'fixture-rev-1', percent: 100 }] };
        return response({ name: `projects/${projectId}/locations/asia-northeast3/operations/fixture` });
      }
      if (method === 'DELETE') { service = null; return response({ name: `projects/${projectId}/locations/asia-northeast3/operations/fixture` }); }
      if (method === 'PATCH') {
        const body = JSON.parse(options.body);
        actions.push({ type: 'patch', value: body.invokerIamDisabled });
        service = { ...service, ...body, ...(body.template ? { latestCreatedRevision: 'fixture-rev-2' } : {}), ...(body.traffic ? { trafficStatuses: body.traffic } : {}) };
        return response({ name: `projects/${projectId}/locations/asia-northeast3/operations/fixture` });
      }
      return service ? response(service) : response({}, 404);
    }
    assert.fail(url);
  } });
  await provider.gateway(id);
  await provider.gatewayReady(id);
  assert.equal(bindingAttempts, 3);
  assert.deepEqual(actions, [{ type: 'create', value: false }, { type: 'patch', value: undefined }, { type: 'patch', value: true }]);
  assert.equal(provider.gatewayUrl(), 'https://fixture.a.run.app');
  assert.equal(vmPolicy.bindings[0].condition.title, 'keep');
  const vmMember = `serviceAccount:momso-vm-${id.slice(0, 12)}@${projectId}.iam.gserviceaccount.com`;
  assert(updatePolicies.some(item => item.url.includes('run.googleapis.com') && item.policy.bindings.some(binding => binding.role.endsWith(`/roles/momsoUpdate${id.slice(0, 12)}`) && binding.members.includes(vmMember))));
  assert(updatePolicies.some(item => item.url.includes('iam.googleapis.com') && item.policy.bindings.some(binding => binding.role === 'roles/iam.serviceAccountUser' && binding.members.includes(vmMember))));
  const vars = service.template.containers[0].env;
  assert.equal(vars.find(item => item.name === 'MOMSO_PRIVATE_CERT_SHA256').value, 'b'.repeat(64));
  assert.equal(vars.find(item => item.name === 'MOMSO_PRIVATE_PROXY_TOKEN').valueSource.secretKeyRef.secret, `momso-transport-${id.slice(0, 12)}`);
  assert(!JSON.stringify(service).includes('fixture-private-token'));
  publicBindings = true;
  await assert.rejects(provider.gateway(id), { step: 'gateway_policy' });
  assert.equal(actions.length, 3);
  publicBindings = false;
  service = null;
  proxyReady = false;
  await assert.rejects(provider.gateway(id), { step: 'proxy_not_running' });
  assert.equal(service, null, 'new owned gateway is removed after failed proxy health');
});

test('address recovery checks owner, trusted running revision and every project page', async () => {
  const image = `registry.example.test/team/gateway@sha256:${'c'.repeat(64)}`;
  const second = 'b'.repeat(24);
  const project = value => ({ projectId: `momso-${value}`, state: 'ACTIVE', labels: { 'momso-install-id': value } });
  const service = (value, owner = '12345') => ({
    name: `projects/momso-${value}/locations/asia-northeast3/services/momso-any-${value.slice(0, 12)}`,
    labels: { 'momso-install-id': value }, invokerIamDisabled: true, generation: '2', observedGeneration: '2',
    terminalCondition: { state: 'CONDITION_SUCCEEDED' }, latestReadyRevision: 'revision-2', latestCreatedRevision: 'revision-2',
    trafficStatuses: [{ revision: 'revision-2', percent: 100 }], uri: `https://fixture-${value[0]}.a.run.app`,
    template: { containers: [{ image, env: [
      { name: 'MOMSO_PUBLIC_HOST', value: `fixture-${value[0]}.a.run.app` },
      { name: 'MOMSO_OWNER_SUB', value: owner },
      { name: 'MOMSO_GOOGLE_CLIENT_ID', value: 'fixture.apps.googleusercontent.com' },
      { name: 'MOMSO_LOGIN_ORIGIN', value: 'https://login.example.test' },
      { name: 'MOMSO_PROJECT', value: `momso-${value}` },
      { name: 'MOMSO_INSTANCE', value: `momso-${value.slice(0, 12)}` },
      { name: 'MOMSO_PRIVATE_HOST', value: '10.42.0.10' },
      { name: 'MOMSO_PRIVATE_PORT', value: '8766' },
      { name: 'MOMSO_PRIVATE_CERT_SHA256', value: 'd'.repeat(64) },
    ] }] },
  });
  let secondOwner = 'another-owner';
  let tamper = false;
  let tamperLogin = false;
  const provider = new GoogleBrowserProvider('fixture', { release: { gatewayImage: image, loginOrigin: 'https://login.example.test' }, releaseVerifier: async value => value, clientId: 'fixture.apps.googleusercontent.com', ownerSub: '12345', fetchImpl: async url => {
    if (url.includes('projects:search')) {
      const page = new URL(url).searchParams.get('pageToken');
      return page ? response({ projects: [project(second)] }) : response({ projects: [project(id)], nextPageToken: 'next' });
    }
    const value = url.includes(`momso-${id}`) ? id : second;
    const item = service(value, value === id ? '12345' : secondOwner);
    if (tamper) item.trafficStatuses[0].revision = 'untrusted-old-revision';
    if (tamperLogin) item.template.containers[0].env.find(x => x.name === 'MOMSO_LOGIN_ORIGIN').value = 'https://attacker.example';
    return response({ services: [item] });
  } });
  assert.deepEqual(await provider.findExisting(), ['https://fixture-a.a.run.app']);
  secondOwner = '12345';
  assert.deepEqual(await provider.findExisting(), ['https://fixture-a.a.run.app', 'https://fixture-b.a.run.app']);
  tamper = true;
  assert.deepEqual(await provider.findExisting(), []);
  tamper = false;
  tamperLogin = true;
  assert.deepEqual(await provider.findExisting(), []);
});

test('address recovery discovers an entry in an existing project without changing project labels', async () => {
  const image = `registry.example.test/team/gateway@sha256:${'c'.repeat(64)}`;
  const name = `momso-any-${id.slice(0, 12)}`;
  let owner = '12345';
  const calls = [];
  const provider = new GoogleBrowserProvider('fixture', { release: { gatewayImage: image, loginOrigin: 'https://login.example.test' }, releaseVerifier: async value => value, clientId: 'fixture.apps.googleusercontent.com', ownerSub: '12345', fetchImpl: async (url, options) => {
    calls.push(options.method);
    if (url.includes('projects:search')) return response({ projects: [{ projectId: 'existing-project', displayName: 'Existing', state: 'ACTIVE' }] });
    return response({ services: [{ name: `projects/existing-project/locations/asia-northeast3/services/${name}`, labels: { 'momso-install-id': id }, invokerIamDisabled: true, generation: '1', observedGeneration: '1', terminalCondition: { state: 'CONDITION_SUCCEEDED' }, latestReadyRevision: 'rev', latestCreatedRevision: 'rev', trafficStatuses: [{ percent: 100, revision: 'rev' }], uri: 'https://fixture.a.run.app', template: { containers: [{ image, env: Object.entries({ MOMSO_PUBLIC_HOST: 'fixture.a.run.app', MOMSO_OWNER_SUB: owner, MOMSO_GOOGLE_CLIENT_ID: 'fixture.apps.googleusercontent.com', MOMSO_LOGIN_ORIGIN: 'https://login.example.test', MOMSO_PROJECT: 'existing-project', MOMSO_INSTANCE: `momso-${id.slice(0, 12)}`, MOMSO_PRIVATE_HOST: '10.42.0.10', MOMSO_PRIVATE_PORT: '8766', MOMSO_PRIVATE_CERT_SHA256: 'd'.repeat(64) }).map(([name, value]) => ({ name, value })) }] } }] });
  } });
  assert.deepEqual(await provider.findExisting(), ['https://fixture.a.run.app']);
  owner = '99999';
  assert.deepEqual(await provider.findExisting(), []);
  assert(calls.every(method => method === 'GET'));
});
