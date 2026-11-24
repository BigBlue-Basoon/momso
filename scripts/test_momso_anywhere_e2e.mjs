import assert from 'node:assert/strict';
import test from 'node:test';
import { GoogleBrowserProvider } from './momso_anywhere_google_browser.mjs';
import { startAnywhere } from './momso_anywhere_setup.mjs';
import { fakeBackupCloud } from './fixture_fake_backup_cloud.mjs';

const ok = (data, status = 200) => ({ ok: status >= 200 && status < 300, status, json: async () => data });

class FakeGoogle {
  constructor(id) {
    this.id = id; this.projectId = `momso-${id}`; this.project = null; this.billing = null;
    this.accounts = new Map(); this.secrets = new Map(); this.policies = new Map();
    this.compute = new Map(); this.roles = new Map(); this.service = null; this.once = true;
    this.created = []; this.vmPolicy = { etag: 'fixture', bindings: [] };
    this.backup = fakeBackupCloud(this.projectId, id, ok);
  }

  async fetch(url, options) {
    if (url === 'https://fixture.a.run.app/__momso/ready') return ok({ ready: true });
    assert.equal(options.headers.Authorization, 'Bearer fixture-oauth');
    const body = options.body ? JSON.parse(options.body) : null;
    const method = options.method;
    const backup = this.backup(url, options);
    if (backup) return backup;
    if (url.includes('cloudresourcemanager.googleapis.com')) {
      if (url.endsWith('/operations/fixture')) return ok({ done: true });
      if (method === 'POST') { this.project = { ...body, state: 'ACTIVE' }; return ok({ name: 'operations/fixture' }); }
      return this.project ? ok(this.project) : ok({}, 404);
    }
    if (url.includes('cloudbilling.googleapis.com')) {
      if (new URL(url).pathname.endsWith('/billingAccounts')) return ok({ billingAccounts: [{ name: 'billingAccounts/FIXTURE', open: true }] });
      if (method === 'PUT') this.billing = body.billingAccountName;
      return ok({ billingEnabled: !!this.billing, ...(this.billing ? { billingAccountName: this.billing } : {}) });
    }
    if (url.includes('serviceusage.googleapis.com')) return method === 'POST' ? ok({ name: 'operations/fixture' }) : ok({ done: true });
    if (url.includes('iam.googleapis.com')) {
      if (url.includes('/serviceAccounts/') && url.includes(':getIamPolicy')) return ok({ etag: 'fixture', bindings: [] });
      if (url.includes('/serviceAccounts/') && url.includes(':setIamPolicy')) return ok(body.policy);
      if (url.includes('/serviceAccounts')) {
        if (method === 'POST') {
          const email = `${body.accountId}@${this.projectId}.iam.gserviceaccount.com`;
          this.accounts.set(email, { email, description: body.serviceAccount.description });
          return ok(this.accounts.get(email));
        }
        const email = url.split('/').at(-1);
        return this.accounts.has(email) ? ok(this.accounts.get(email)) : ok({}, 404);
      }
      if (url.includes('/roles')) {
        if (method === 'POST') { this.roles.set(body.roleId, body.role); return ok(body.role); }
        const role = this.roles.get(url.split('/').at(-1));
        return role ? ok(role) : ok({}, 404);
      }
    }
    if (url.includes('secretmanager.googleapis.com')) {
      if (url.includes('?secretId=')) { const name = url.split('?secretId=')[1]; this.secrets.set(name, { labels: { 'momso-install-id': this.id }, versions: [] }); return ok({}); }
      const name = url.split('/secrets/')[1].split(/[/:?]/)[0];
      if (url.includes(':getIamPolicy')) return ok(this.policies.get(name) ?? { etag: 'fixture', bindings: [] });
      if (url.includes(':setIamPolicy')) { this.policies.set(name, body.policy); return ok({}); }
      if (url.includes(':addVersion')) { assert(body.payload.data); this.secrets.get(name).versions.push({ state: 'ENABLED' }); return ok({}); }
      if (url.endsWith('/versions/1')) return this.secrets.get(name).versions[0] ? ok(this.secrets.get(name).versions[0]) : ok({}, 404);
      if (url.endsWith('/versions')) return ok({ versions: this.secrets.get(name).versions });
      return this.secrets.has(name) ? ok(this.secrets.get(name)) : ok({}, 404);
    }
    if (url.includes('compute.googleapis.com')) {
      if (url.includes('/operations/')) return ok({ status: 'DONE' });
      if (url.includes('/getGuestAttributes')) { const vault = this.secrets.get(`momso-vault-${this.id.slice(0, 12)}`); if (vault && !vault.versions.length) vault.versions.push({ state: 'ENABLED' }); return ok({ queryValue: { items: [{ namespace: 'momso', key: 'fingerprint', value: 'b'.repeat(64) }] } }); }
      if (url.includes('/getIamPolicy')) return ok(this.vmPolicy);
      if (url.includes('/setIamPolicy')) { this.vmPolicy = body.policy; return ok(this.vmPolicy); }
      const base = `https://compute.googleapis.com/compute/v1/projects/${this.projectId}/`;
      const path = url.slice(base.length);
      if (method === 'GET') return this.compute.has(path) ? ok(this.compute.get(path)) : ok({}, 404);
      if (method === 'POST') {
        if (path.endsWith('/instances') && this.once) { this.once = false; return ok({}, 503); }
        const key = `${path}/${body.name}`;
        this.compute.set(key, path.endsWith('/instances') ? { ...body, status: 'RUNNING' } : body);
        this.created.push(key);
        const location = path.startsWith('zones/') ? { zone: `projects/fixture/zones/asia-northeast3-a` } : path.startsWith('regions/') ? { region: `projects/fixture/regions/asia-northeast3` } : {};
        return ok({ name: 'fixture-op', ...location });
      }
    }
    if (url.includes('run.googleapis.com')) {
      if (url.includes('/operations/')) return ok({ done: true });
      if (url.includes('/revisions/')) return ok({ conditions: [{ type: 'Ready', state: 'CONDITION_SUCCEEDED', revisionReason: 'RETIRED' }], containers: [{ image: this.service.template.containers[0].image }] });
      if (url.includes(':getIamPolicy')) return ok({ etag: 'fixture', bindings: [] });
      if (url.includes(':setIamPolicy')) return ok(body.policy);
      const operation = { name: `projects/${this.projectId}/locations/asia-northeast3/operations/fixture` };
      if (method === 'POST') {
        this.service = { ...body, uri: 'https://fixture.a.run.app', etag: 'fixture', terminalCondition: { state: 'CONDITION_SUCCEEDED' }, latestReadyRevision: 'fixture-rev-1', latestCreatedRevision: 'fixture-rev-1', trafficStatuses: [{ revision: 'fixture-rev-1', percent: 100 }] };
        return ok(operation);
      }
      if (method === 'PATCH') { this.service = { ...this.service, ...body, ...(body.template ? { latestCreatedRevision: 'fixture-rev-2' } : {}), ...(body.traffic ? { trafficStatuses: body.traffic } : {}) }; return ok(operation); }
      return this.service ? ok(this.service) : ok({}, 404);
    }
    assert.fail(`unexpected API request: ${method} ${url}`);
  }
}

test('browser setup resumes after VM creation failure using customer-owned resources', async () => {
  const id = 'd'.repeat(24);
  const checkpoint = { installationId: id };
  const cloud = new FakeGoogle(id);
  const release = { bundleUrl: 'https://example.test/release.tar.gz', bundleSha256: 'e'.repeat(64), gatewayImage: `ghcr.io/example/momso-gateway@sha256:${'f'.repeat(64)}`, loginOrigin: 'https://login.example.test' };
  const provider = new GoogleBrowserProvider('fixture-oauth', { fetchImpl: cloud.fetch.bind(cloud), sleep: async () => {}, release, releaseVerifier: async value => value, clientId: 'fixture.apps.googleusercontent.com', ownerSub: '12345' });
  const progress = [];
  await assert.rejects(startAnywhere(provider, checkpoint, value => progress.push(value)), { step: 'google_unavailable' });
  assert(progress.some(item => item.step === 'server' && item.state === 'failed'));
  const result = await startAnywhere(provider, checkpoint);
  assert.deepEqual(result, { url: 'https://fixture.a.run.app' });
  assert.equal(new Set(cloud.created).size, cloud.created.length);
  assert.equal(cloud.secrets.size, 3);
  assert.equal(cloud.service.invokerIamDisabled, true);
  assert.equal(cloud.service.template.containers[0].image, release.gatewayImage);
  assert(!JSON.stringify(checkpoint).includes('fixture-oauth'));
  assert(!JSON.stringify(cloud.service).includes('fixture-oauth'));
});
