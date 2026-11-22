import { SetupError } from './momso_anywhere_setup.mjs';
import { vmStartupScript } from './momso_anywhere_vm_startup.mjs';
import { prepareBackupResources } from './momso_anywhere_backup_setup.mjs';
import { activatePinnedRevision } from './momso_anywhere_cloud_run_revision.mjs';
import { verifyRelease } from './momso_release_manifest.mjs';

const CRM = 'https://cloudresourcemanager.googleapis.com/v3/';
const BILLING = 'https://cloudbilling.googleapis.com/v1/';
const USAGE = 'https://serviceusage.googleapis.com/v1/';
const COMPUTE = 'https://compute.googleapis.com/compute/v1/';
const IAM = 'https://iam.googleapis.com/v1/';
const SECRETS = 'https://secretmanager.googleapis.com/v1/';
const RUN = 'https://run.googleapis.com/v2/';
const REGION = 'asia-northeast3';
const ZONE = 'asia-northeast3-a';
const SERVICES = ['compute.googleapis.com', 'run.googleapis.com', 'secretmanager.googleapis.com', 'iam.googleapis.com', 'serviceusage.googleapis.com', 'storage.googleapis.com', 'cloudkms.googleapis.com'];

export async function providerFromToken({ token, expiresAt, clientId, release, billingAccountName = null, projectId = null, fetchImpl = fetch }) {
  if (!token || !Number.isFinite(expiresAt) || expiresAt <= Date.now()) throw new SetupError('authorization_expired');
  let response;
  try { response = await fetchImpl('https://openidconnect.googleapis.com/v1/userinfo', { headers: { Authorization: `Bearer ${token}` }, cache: 'no-store', redirect: 'error' }); }
  catch { throw new SetupError('identity_unavailable'); }
  if (!response.ok) throw new SetupError(response.status === 401 ? 'authorization_expired' : 'identity_unavailable');
  const identity = await response.json();
  if (!/^[0-9]{1,40}$/.test(identity.sub ?? '')) throw new SetupError('identity_unavailable');
  return { ownerSub: identity.sub, provider: new GoogleBrowserProvider(token, { fetchImpl, release, clientId, ownerSub: identity.sub, billingAccountName, projectId, expiresAt }) };
}

// The access token exists only in this instance's private memory.
export class GoogleBrowserProvider {
  #token;
  #fetch;
  #sleep;
  #release;
  #clientId;
  #ownerSub;
  #gatewayUrl;
  #billingAccountName;
  #selectedProject;
  #expiresAt;
  #releaseConfig;
  #releaseVerifier;
  constructor(accessToken, { fetchImpl = fetch, sleep = ms => new Promise(resolve => setTimeout(resolve, ms)), release = null, clientId = null, ownerSub = null, billingAccountName = null, projectId = null, expiresAt = Infinity, releaseVerifier = verifyRelease } = {}) {
    if (!accessToken) throw new SetupError('invalid_authorization');
    this.#token = accessToken;
    this.#fetch = fetchImpl;
    this.#sleep = sleep;
    this.#release = null;
    this.#releaseConfig = release;
    this.#releaseVerifier = releaseVerifier;
    this.#clientId = clientId;
    this.#ownerSub = ownerSub;
    if (billingAccountName !== null && !/^billingAccounts\/[A-Za-z0-9-]+$/.test(billingAccountName)) throw new SetupError('invalid_billing_account');
    this.#billingAccountName = billingAccountName;
    if (projectId !== null && !/^[a-z][a-z0-9-]{4,28}[a-z0-9]$/.test(projectId)) throw new SetupError('invalid_project');
    this.#selectedProject = projectId;
    this.#expiresAt = expiresAt;
  }

  async #request(url, { method = 'GET', body, allowMissing = false, missingForbidden = false } = {}) {
    if (Date.now() >= this.#expiresAt) throw new SetupError('authorization_expired');
    if (![CRM, BILLING, USAGE, COMPUTE, IAM, SECRETS, RUN, 'https://storage.googleapis.com/storage/v1/', 'https://cloudkms.googleapis.com/v1/'].some(base => url.startsWith(base))) throw new SetupError('invalid_api');
    let response;
    try {
      response = await this.#fetch(url, { method, headers: { Authorization: `Bearer ${this.#token}`, ...(body ? { 'Content-Type': 'application/json' } : {}) }, ...(body ? { body: JSON.stringify(body) } : {}), cache: 'no-store', redirect: 'error' });
    } catch { throw new SetupError('network'); }
    if ((allowMissing && response.status === 404) || (missingForbidden && response.status === 403)) return null;
    if (!response.ok) {
      const reason = response.status === 401 ? 'authorization_expired' : response.status === 403 ? 'permission_denied' : response.status === 402 ? 'billing_setup_required' : response.status === 429 ? 'quota_exceeded' : response.status >= 500 ? 'google_unavailable' : 'google_request';
      throw new SetupError(reason);
    }
    if (response.status === 204) return {};
    try { return await response.json(); } catch { throw new SetupError('google_response'); }
  }

  async #poll(base, name) {
    if (!/^operations\/[A-Za-z0-9._-]+$/.test(name)) throw new SetupError('invalid_operation');
    for (let i = 0; i < 120; i++) {
      const operation = await this.#request(base + name);
      if (operation.done) {
        if (operation.error) throw new SetupError('operation_failed');
        return operation.response;
      }
      await this.#sleep(1000);
    }
    throw new SetupError('operation_timeout');
  }

  async #retryPropagation(action) {
    for (let attempt = 0; attempt < 6; attempt++) {
      try { return await action(); }
      catch (error) {
        if (!(error instanceof SetupError) || error.step !== 'google_request' || attempt === 5) throw error;
        await this.#sleep(5000);
      }
    }
  }

  #projectId(id) {
    if (!/^[a-f0-9]{24}$/.test(id)) throw new SetupError('invalid_checkpoint');
    return this.#selectedProject ?? `momso-${id}`;
  }

  async projects() {
    const projects = [];
    const seen = new Set();
    let page = '';
    do {
      if (seen.has(page)) throw new SetupError('google_response');
      seen.add(page);
      const query = new URLSearchParams({ query: 'state:ACTIVE', pageSize: '100' });
      if (page) query.set('pageToken', page);
      const result = await this.#request(`${CRM}projects:search?${query}`);
      for (const item of result.projects ?? []) {
        if (item.state === 'ACTIVE' && /^[a-z][a-z0-9-]{4,28}[a-z0-9]$/.test(item.projectId ?? '')) projects.push({ projectId: item.projectId, displayName: String(item.displayName ?? '') });
      }
      page = result.nextPageToken ?? '';
    } while (page);
    return projects;
  }

  async preflight() {
    try {
      if (!this.#release) this.#release = await this.#releaseVerifier(this.#releaseConfig, { fetchImpl: this.#fetch });
      vmStartupScript({ project: 'fixture-project', secret: 'momso-transport-aaaaaaaaaaaa', bundleUrl: this.#release.bundleUrl, bundleSha256: this.#release.bundleSha256 });
      const origin = new URL(this.#release.loginOrigin);
      if (origin.protocol !== 'https:' || origin.origin !== this.#release.loginOrigin || !/^[a-z0-9.-]+\/[a-z0-9._/-]+@sha256:[a-f0-9]{64}$/.test(this.#release.gatewayImage) || !/^[A-Za-z0-9._-]+\.apps\.googleusercontent\.com$/.test(this.#clientId ?? '') || !/^[0-9]{1,40}$/.test(this.#ownerSub ?? '')) throw new Error();
    } catch { throw new SetupError('release_not_ready'); }
  }

  gatewayUrl() { return this.#gatewayUrl ?? null; }

  async findExisting() {
    if (!/^[0-9]{1,40}$/.test(this.#ownerSub ?? '') || !/^[A-Za-z0-9._-]+\.apps\.googleusercontent\.com$/.test(this.#clientId ?? '')) throw new SetupError('identity_unavailable');
    if (!this.#release) this.#release = await this.#releaseVerifier(this.#releaseConfig, { fetchImpl: this.#fetch });
    let loginOrigin;
    try {
      const origin = new URL(this.#release?.loginOrigin);
      if (origin.protocol !== 'https:' || origin.origin !== this.#release.loginOrigin) throw new Error();
      loginOrigin = origin.origin;
    } catch { throw new SetupError('release_not_ready'); }
    const trusted = [this.#release?.gatewayImage, ...(this.#release?.trustedGatewayImages ?? [])];
    if (!trusted.length || trusted.some(value => !/^[a-z0-9.-]+\/[a-z0-9._/-]+@sha256:[a-f0-9]{64}$/.test(value ?? ''))) throw new SetupError('release_not_ready');
    const results = new Set();
    for (const project of await this.projects()) {
      const seen = new Set();
      let page = '';
      do {
        if (seen.has(page)) throw new SetupError('google_response');
        seen.add(page);
        const query = new URLSearchParams({ pageSize: '100' });
        if (page) query.set('pageToken', page);
        const found = await this.#request(`${RUN}projects/${project.projectId}/locations/${REGION}/services?${query}`, { allowMissing: true, missingForbidden: true });
        for (const service of found?.services ?? []) {
        const id = service.labels?.['momso-install-id'];
        if (!/^[a-f0-9]{24}$/.test(id ?? '') || service.name !== `projects/${project.projectId}/locations/${REGION}/services/momso-any-${id.slice(0, 12)}`) continue;
        if (!service || service.labels?.['momso-install-id'] !== id || service.reconciling || service.terminalCondition?.state !== 'CONDITION_SUCCEEDED' || !service.invokerIamDisabled || service.iapEnabled || !service.latestReadyRevision || service.latestReadyRevision !== service.latestCreatedRevision || service.generation == null || service.observedGeneration == null || String(service.observedGeneration) !== String(service.generation)) continue;
        const traffic = service.trafficStatuses ?? [];
        if (traffic.length !== 1 || traffic[0].percent !== 100 || traffic[0].revision !== service.latestReadyRevision) continue;
        const containers = service.template?.containers ?? [];
        if (containers.length !== 1 || !trusted.includes(containers[0].image)) continue;
        const env = Object.fromEntries((containers[0].env ?? []).filter(item => typeof item.value === 'string').map(item => [item.name, item.value]));
        let uri;
        try { uri = new URL(service.uri); } catch { continue; }
        if (uri.protocol !== 'https:' || uri.origin !== service.uri || !uri.hostname.endsWith('.run.app') || env.MOMSO_PUBLIC_HOST !== uri.hostname || env.MOMSO_OWNER_SUB !== this.#ownerSub || env.MOMSO_GOOGLE_CLIENT_ID !== this.#clientId || env.MOMSO_LOGIN_ORIGIN !== loginOrigin || env.MOMSO_PROJECT !== project.projectId || env.MOMSO_INSTANCE !== `momso-${id.slice(0, 12)}` || env.MOMSO_PRIVATE_HOST !== '10.42.0.10' || env.MOMSO_PRIVATE_PORT !== '8766' || !/^[a-f0-9]{64}$/.test(env.MOMSO_PRIVATE_CERT_SHA256 ?? '')) continue;
        results.add(uri.origin);
        }
        page = found?.nextPageToken ?? '';
      } while (page);
    }
    return [...results].sort();
  }

  async project(id) {
    const projectId = this.#projectId(id);
    // A missing Google project can be indistinguishable from a forbidden project.
    // Creating the globally unique ID will fail, not adopt it, if another owner has it.
    let current = await this.#request(`${CRM}projects/${projectId}`, { allowMissing: true, missingForbidden: true });
    if (!current) {
      if (this.#selectedProject) throw new SetupError('project_unavailable');
      const operation = await this.#request(`${CRM}projects`, { method: 'POST', body: { projectId, displayName: 'momso', labels: { 'momso-install-id': id } } });
      await this.#poll(CRM, operation.name);
      current = await this.#request(`${CRM}projects/${projectId}`);
    }
    if (current.projectId !== projectId || (!this.#selectedProject && current.labels?.['momso-install-id'] !== id) || current.state !== 'ACTIVE') throw new SetupError('project_ownership');
  }

  async billingAccounts() {
    const accounts = [];
    const seen = new Set();
    let page = '';
    do {
      if (seen.has(page)) throw new SetupError('google_response');
      seen.add(page);
      const query = new URLSearchParams({ pageSize: '100' });
      if (page) query.set('pageToken', page);
      const result = await this.#request(`${BILLING}billingAccounts?${query}`);
      for (const item of result.billingAccounts ?? []) {
        if (item.open && /^billingAccounts\/[A-Za-z0-9-]+$/.test(item.name)) accounts.push({ name: item.name, displayName: String(item.displayName ?? '') });
      }
      page = result.nextPageToken ?? '';
    } while (page);
    return accounts;
  }

  async billing(id) {
    const projectId = this.#projectId(id);
    const current = await this.#request(`${BILLING}projects/${projectId}/billingInfo`);
    if (current.billingEnabled) return;
    if (current.billingAccountName) throw new SetupError('billing_inactive');
    const open = await this.billingAccounts();
    const selected = this.#billingAccountName ? open.find(item => item.name === this.#billingAccountName) : open.length === 1 ? open[0] : null;
    if (!selected) throw new SetupError(this.#billingAccountName ? 'billing_account_unavailable' : open.length ? 'billing_selection_required' : 'billing_setup_required');
    await this.#request(`${BILLING}projects/${projectId}/billingInfo`, { method: 'PUT', body: { billingAccountName: selected.name } });
    if (!(await this.#request(`${BILLING}projects/${projectId}/billingInfo`)).billingEnabled) throw new SetupError('billing_not_ready');
  }

  async services(id) {
    const projectId = this.#projectId(id);
    const result = await this.#request(`${USAGE}projects/${projectId}/services:batchEnable`, { method: 'POST', body: { serviceIds: SERVICES } });
    await this.#poll(USAGE, result.name);
  }

  async #computeOperation(projectId, operation) {
    if (!/^[a-zA-Z0-9_-]+$/.test(operation.name)) throw new SetupError('invalid_operation');
    if ((operation.region && !operation.region.endsWith('/' + REGION)) || (operation.zone && !operation.zone.endsWith('/' + ZONE))) throw new SetupError('invalid_operation');
    const scope = operation.zone ? `zones/${ZONE}` : operation.region ? `regions/${REGION}` : 'global';
    for (let i = 0; i < 120; i++) {
      const current = await this.#request(`${COMPUTE}projects/${projectId}/${scope}/operations/${operation.name}`);
      if (current.status === 'DONE') {
        if (current.error) throw new SetupError('operation_failed');
        return;
      }
      await this.#sleep(1000);
    }
    throw new SetupError('operation_timeout');
  }

  async #ensure(projectId, resourcePath, listPath, desired, matches) {
    let found = await this.#request(`${COMPUTE}projects/${projectId}/${resourcePath}`, { allowMissing: true });
    if (!found) {
      const op = await this.#request(`${COMPUTE}projects/${projectId}/${listPath}`, { method: 'POST', body: desired });
      await this.#computeOperation(projectId, op);
      found = await this.#request(`${COMPUTE}projects/${projectId}/${resourcePath}`);
    }
    if (!matches(found)) throw new SetupError('resource_ownership');
  }

  async privateNetwork(id) {
    const projectId = this.#projectId(id);
    const name = `momso-${id.slice(0, 12)}`;
    const marker = `momso-install-${id}`;
    const network = `${name}-net`;
    const netUrl = `projects/${projectId}/global/networks/${network}`;
    await this.#ensure(projectId, `global/networks/${network}`, 'global/networks', { name: network, description: marker, autoCreateSubnetworks: false }, x => x.description === marker && x.autoCreateSubnetworks === false);
    const subnet = `${name}-sub`;
    await this.#ensure(projectId, `regions/${REGION}/subnetworks/${subnet}`, `regions/${REGION}/subnetworks`, { name: subnet, description: marker, network: netUrl, ipCidrRange: '10.42.0.0/24' }, x => x.description === marker && x.network?.endsWith('/' + network) && x.ipCidrRange === '10.42.0.0/24');
    const rule = `${name}-private`;
    await this.#ensure(projectId, `global/firewalls/${rule}`, 'global/firewalls', { name: rule, description: marker, network: netUrl, direction: 'INGRESS', sourceRanges: ['10.42.0.0/24'], targetTags: [name], allowed: [{ IPProtocol: 'tcp', ports: ['8766'] }] }, x => x.description === marker && x.network?.endsWith('/' + network) && x.direction === 'INGRESS' && !x.disabled && JSON.stringify(x.sourceRanges) === '["10.42.0.0/24"]' && JSON.stringify(x.targetTags) === JSON.stringify([name]) && JSON.stringify(x.allowed) === JSON.stringify([{ IPProtocol: 'tcp', ports: ['8766'] }]));
    const router = `${name}-router`;
    await this.#ensure(projectId, `regions/${REGION}/routers/${router}`, `regions/${REGION}/routers`, { name: router, description: marker, network: netUrl, nats: [{ name: `${name}-nat`, natIpAllocateOption: 'AUTO_ONLY', sourceSubnetworkIpRangesToNat: 'ALL_SUBNETWORKS_ALL_IP_RANGES' }] }, x => x.description === marker && x.network?.endsWith('/' + network) && x.nats?.length === 1 && x.nats[0].name === `${name}-nat` && x.nats[0].natIpAllocateOption === 'AUTO_ONLY' && x.nats[0].sourceSubnetworkIpRangesToNat === 'ALL_SUBNETWORKS_ALL_IP_RANGES');
  }

  async preparePrivateIdentity(id) {
    const projectId = this.#projectId(id);
    const project = await this.#request(`${CRM}projects/${projectId}`);
    if (project.projectId !== projectId || (!this.#selectedProject && project.labels?.['momso-install-id'] !== id) || project.state !== 'ACTIVE') throw new SetupError('project_ownership');
    const short = id.slice(0, 12);
    const marker = `momso-install-${id}`;
    for (const suffix of ['vm', 'gateway']) {
      const accountId = `momso-${suffix}-${short}`;
      const email = `${accountId}@${projectId}.iam.gserviceaccount.com`;
      const path = `${IAM}projects/${projectId}/serviceAccounts/${email}`;
      let account = await this.#request(path, { allowMissing: true });
      if (!account) {
        await this.#request(`${IAM}projects/${projectId}/serviceAccounts`, { method: 'POST', body: { accountId, serviceAccount: { displayName: `momso ${suffix}`, description: marker } } });
        account = await this.#request(path);
      }
      if (account.email !== email || account.description !== marker || account.disabled) throw new SetupError('identity_ownership');
    }
    for (const suffix of ['transport', 'session', 'vault']) {
      const secret = `momso-${suffix}-${short}`;
      const base = `${SECRETS}projects/${projectId}/secrets/${secret}`;
      let item = await this.#request(base, { allowMissing: true });
      if (!item) {
        await this.#request(`${SECRETS}projects/${projectId}/secrets?secretId=${secret}`, { method: 'POST', body: { replication: { automatic: {} }, labels: { 'momso-install-id': id } } });
        item = await this.#request(base);
      }
      if (item.labels?.['momso-install-id'] !== id) throw new SetupError('secret_ownership');
      const versions = await this.#request(`${base}/versions`);
      if (versions.nextPageToken) throw new SetupError('secret_versions');
      if (!(versions.versions ?? []).length && suffix !== 'vault') {
        const random = crypto.getRandomValues(new Uint8Array(48));
        const value = btoa(String.fromCharCode(...random)).replaceAll('+', '-').replaceAll('/', '_').replace(/=+$/, '');
        await this.#request(`${base}:addVersion`, { method: 'POST', body: { payload: { data: btoa(value) } } });
      } else if ((versions.versions ?? []).length && !(versions.versions ?? []).some(version => version.state === 'ENABLED')) {
        throw new SetupError('secret_versions');
      }
      const vmMember = `serviceAccount:momso-vm-${short}@${projectId}.iam.gserviceaccount.com`;
      const members = suffix === 'vault' ? [vmMember] : [`serviceAccount:momso-gateway-${short}@${projectId}.iam.gserviceaccount.com`];
      if (suffix === 'transport') members.push(`serviceAccount:momso-vm-${short}@${projectId}.iam.gserviceaccount.com`);
      const policy = await this.#request(`${base}:getIamPolicy?options.requestedPolicyVersion=3`);
      if (!policy.etag) throw new SetupError('policy_unavailable');
      const bindings = policy.bindings ?? [];
      let binding = bindings.find(x => x.role === 'roles/secretmanager.secretAccessor' && !x.condition);
      if (!binding) { binding = { role: 'roles/secretmanager.secretAccessor', members: [] }; bindings.push(binding); }
      binding.members = [...new Set([...(binding.members ?? []), ...members])];
      if (suffix === 'vault' && !(versions.versions ?? []).length) {
        const conditionTitle = `momso-vault-init-${short}`;
        let adder = bindings.find(x => x.role === 'roles/secretmanager.secretVersionAdder' && x.condition?.title === conditionTitle);
        if (!adder) { adder = { role: 'roles/secretmanager.secretVersionAdder', members: [], condition: { title: conditionTitle } }; bindings.push(adder); }
        if ((adder.members ?? []).some(member => member !== vmMember)) throw new SetupError('policy_unavailable');
        adder.condition.expression = `request.time < timestamp("${new Date(Date.now() + 15 * 60 * 1000).toISOString()}")`;
        adder.members = [...new Set([...(adder.members ?? []), vmMember])];
      }
      await this.#retryPropagation(() => this.#request(`${base}:setIamPolicy`, { method: 'POST', body: { policy: { ...policy, version: 3, bindings } } }));
    }
  }

  async server(id) {
    await this.preflight();
    const projectId = this.#projectId(id);
    if (!this.#release) throw new SetupError('release_not_ready');
    const short = id.slice(0, 12);
    const name = `momso-${short}`;
    const account = `momso-vm-${short}@${projectId}.iam.gserviceaccount.com`;
    const script = vmStartupScript({ project: projectId, secret: `momso-transport-${short}`, bundleUrl: this.#release.bundleUrl, bundleSha256: this.#release.bundleSha256 });
    let identityPrepared = false;
    try {
    await this.preparePrivateIdentity(id);
    identityPrepared = true;
    await prepareBackupResources((url, options) => this.#request(url, options), projectId, id);
    const path = `zones/${ZONE}/instances/${name}`;
    const subnet = `projects/${projectId}/regions/${REGION}/subnetworks/${name}-sub`;
    const metadata = [{ key: 'startup-script', value: script }, { key: 'enable-guest-attributes', value: 'TRUE' }, { key: 'block-project-ssh-keys', value: 'TRUE' }];
    const desired = { name, machineType: `zones/${ZONE}/machineTypes/e2-small`, labels: { 'momso-install-id': id }, tags: { items: [name] }, metadata: { items: metadata }, networkInterfaces: [{ subnetwork: subnet, networkIP: '10.42.0.10' }], serviceAccounts: [{ email: account, scopes: ['https://www.googleapis.com/auth/cloud-platform'] }], disks: [{ boot: true, autoDelete: true, initializeParams: { sourceImage: 'projects/debian-cloud/global/images/family/debian-12', diskSizeGb: '20' } }] };
    await this.#ensure(projectId, path, `zones/${ZONE}/instances`, desired, x => x.labels?.['momso-install-id'] === id && x.networkInterfaces?.length === 1 && x.networkInterfaces[0].networkIP === '10.42.0.10' && !x.networkInterfaces[0].accessConfigs?.length && !x.networkInterfaces[0].ipv6AccessConfigs?.length && x.networkInterfaces[0].subnetwork?.endsWith('/' + name + '-sub') && x.tags?.items?.includes(name) && x.serviceAccounts?.length === 1 && x.serviceAccounts[0].email === account && x.metadata?.items?.some(item => item.key === 'startup-script' && item.value === script) && x.metadata?.items?.some(item => item.key === 'block-project-ssh-keys' && item.value === 'TRUE') && x.metadata?.items?.some(item => item.key === 'enable-guest-attributes' && item.value === 'TRUE'));
    await this.#grantVmStop(projectId, id, name, account);
    const instance = await this.#request(`${COMPUTE}projects/${projectId}/${path}`);
    if (instance.status === 'TERMINATED') {
      const op = await this.#request(`${COMPUTE}projects/${projectId}/${path}/start`, { method: 'POST' });
      await this.#computeOperation(projectId, op);
    }
    for (let i = 0; i < 180; i++) {
      try {
        const attrs = await this.#request(`${COMPUTE}projects/${projectId}/${path}/getGuestAttributes?queryPath=momso%2Ffingerprint`);
        const fingerprint = attrs.queryValue?.items?.find(item => item.namespace === 'momso' && item.key === 'fingerprint')?.value;
        if (/^[a-f0-9]{64}$/.test(fingerprint ?? '')) {
          await this.#revokeVaultAdder(projectId, short, account, true);
          identityPrepared = false;
          return;
        }
      } catch (error) {
        if (!(error instanceof SetupError)) throw error;
      }
      await this.#sleep(1000);
    }
    throw new SetupError('server_not_ready');
    } finally {
      if (identityPrepared) await this.#revokeVaultAdder(projectId, short, account, false);
    }
  }

  async #grantVmStop(projectId, id, instance, account) {
    const roleId = `momsoStop${id.slice(0, 12)}`;
    const rolePath = `${IAM}projects/${projectId}/roles/${roleId}`;
    const marker = `momso-install-${id}`;
    let role = await this.#request(rolePath, { allowMissing: true });
    if (!role) {
      await this.#request(`${IAM}projects/${projectId}/roles`, { method: 'POST', body: { roleId, role: { title: roleId, description: marker, includedPermissions: ['compute.instances.stop'], stage: 'GA' } } });
      role = await this.#request(rolePath);
    }
    if (role.description !== marker || role.deleted || JSON.stringify(role.includedPermissions) !== '["compute.instances.stop"]') throw new SetupError('role_ownership');
    const endpoint = `${COMPUTE}projects/${projectId}/zones/${ZONE}/instances/${instance}`;
    const policy = await this.#request(`${endpoint}/getIamPolicy?optionsRequestedPolicyVersion=3`);
    if (!policy.etag) throw new SetupError('policy_unavailable');
    const bindings = policy.bindings ?? [];
    const roleName = `projects/${projectId}/roles/${roleId}`;
    let binding = bindings.find(value => value.role === roleName && !value.condition);
    if (!binding) { binding = { role: roleName, members: [] }; bindings.push(binding); }
    binding.members = [...new Set([...(binding.members ?? []), `serviceAccount:${account}`])];
    await this.#retryPropagation(() => this.#request(`${endpoint}/setIamPolicy`, { method: 'POST', body: { policy: { ...policy, version: 3, bindings } } }));
  }

  async #revokeVaultAdder(projectId, short, account, requireVersion) {
    const base = `${SECRETS}projects/${projectId}/secrets/momso-vault-${short}`;
    if (requireVersion) {
      const initialVersion = await this.#request(`${base}/versions/1`, { allowMissing: true });
      if (initialVersion?.state !== 'ENABLED') throw new SetupError('vault_not_ready');
    }
    const policy = await this.#request(`${base}:getIamPolicy?options.requestedPolicyVersion=3`);
    if (!policy.etag) throw new SetupError('policy_unavailable');
    const member = `serviceAccount:${account}`;
    const bindings = (policy.bindings ?? []).map(value => value.role === 'roles/secretmanager.secretVersionAdder' && value.condition?.title === `momso-vault-init-${short}` ? { ...value, members: (value.members ?? []).filter(item => item !== member) } : value).filter(value => value.members?.length);
    await this.#request(`${base}:setIamPolicy`, { method: 'POST', body: { policy: { ...policy, version: 3, bindings } } });
  }

  async #pollRun(projectId, name) {
    const prefix = `projects/${projectId}/locations/${REGION}/operations/`;
    if (!name?.startsWith(prefix) || !/^[A-Za-z0-9._-]+$/.test(name.slice(prefix.length))) throw new SetupError('invalid_operation');
    for (let i = 0; i < 180; i++) {
      const operation = await this.#request(RUN + name);
      if (operation.done) {
        if (operation.error) throw new SetupError('operation_failed');
        return;
      }
      await this.#sleep(1000);
    }
    throw new SetupError('operation_timeout');
  }

  async gateway(id) {
    await this.preflight();
    const projectId = this.#projectId(id);
    const short = id.slice(0, 12);
    const name = `momso-${short}`;
    const marker = `momso-install-${id}`;
    const account = `momso-gateway-${short}@${projectId}.iam.gserviceaccount.com`;
    const instance = `${COMPUTE}projects/${projectId}/zones/${ZONE}/instances/${name}`;
    const attrs = await this.#request(`${instance}/getGuestAttributes?queryPath=momso%2Ffingerprint`);
    const pin = attrs.queryValue?.items?.find(item => item.namespace === 'momso' && item.key === 'fingerprint')?.value;
    if (!/^[a-f0-9]{64}$/.test(pin ?? '')) throw new SetupError('server_not_ready');
    const roleId = `momsoAnywhere${short}`;
    const rolePath = `${IAM}projects/${projectId}/roles/${roleId}`;
    let role = await this.#request(rolePath, { allowMissing: true });
    if (!role) {
      await this.#request(`${IAM}projects/${projectId}/roles`, { method: 'POST', body: { roleId, role: { title: roleId, description: marker, includedPermissions: ['compute.instances.get', 'compute.instances.start'], stage: 'GA' } } });
      role = await this.#request(rolePath);
    }
    const permissions = [...(role.includedPermissions ?? [])].sort();
    if (role.description !== marker || role.deleted || JSON.stringify(permissions) !== JSON.stringify(['compute.instances.get', 'compute.instances.start'])) throw new SetupError('role_ownership');
    const policy = await this.#request(`${instance}/getIamPolicy?optionsRequestedPolicyVersion=3`);
    if (!policy.etag) throw new SetupError('policy_unavailable');
    const roleName = `projects/${projectId}/roles/${roleId}`;
    const bindings = policy.bindings ?? [];
    let binding = bindings.find(item => item.role === roleName && !item.condition);
    if (!binding) { binding = { role: roleName, members: [] }; bindings.push(binding); }
    binding.members = [...new Set([...(binding.members ?? []), `serviceAccount:${account}`])];
    await this.#retryPropagation(() => this.#request(`${instance}/setIamPolicy`, { method: 'POST', body: { policy: { ...policy, version: 3, bindings } } }));
    const serviceId = `momso-any-${short}`;
    const serviceName = `projects/${projectId}/locations/${REGION}/services/${serviceId}`;
    const endpoint = RUN + serviceName;
    const env = host => Object.entries({ MOMSO_PRIVATE_HOST: '10.42.0.10', MOMSO_PRIVATE_PORT: '8766', MOMSO_PRIVATE_CERT_SHA256: pin, MOMSO_PROJECT: projectId, MOMSO_ZONE: ZONE, MOMSO_INSTANCE: name, MOMSO_GOOGLE_CLIENT_ID: this.#clientId, MOMSO_OWNER_SUB: this.#ownerSub, MOMSO_LOGIN_ORIGIN: this.#release.loginOrigin, MOMSO_SESSION_KEY_FILE: '/secrets/session-key', MOMSO_PUBLIC_HOST: host }).map(([key, value]) => ({ name: key, value }));
    const template = host => ({ serviceAccount: account, timeout: '3600s', vpcAccess: { networkInterfaces: [{ network: `${name}-net`, subnetwork: `${name}-sub` }], egress: 'PRIVATE_RANGES_ONLY' }, volumes: [{ name: 'session', secret: { secret: `momso-session-${short}`, items: [{ version: 'latest', path: 'session-key' }] } }], containers: [{ image: this.#release.gatewayImage, env: [...env(host), { name: 'MOMSO_PRIVATE_PROXY_TOKEN', valueSource: { secretKeyRef: { secret: `momso-transport-${short}`, version: 'latest' } } }], volumeMounts: [{ name: 'session', mountPath: '/secrets' }] }] });
    let service = await this.#request(endpoint, { allowMissing: true });
    if (service && service.labels?.['momso-install-id'] !== id) throw new SetupError('gateway_ownership');
    const previousPin = service?.template?.containers?.[0]?.env?.find(item => item.name === 'MOMSO_PRIVATE_CERT_SHA256')?.value;
    if (previousPin && previousPin !== pin) throw new SetupError('certificate_changed');
    if (!service) {
      const operation = await this.#request(`${RUN}projects/${projectId}/locations/${REGION}/services?serviceId=${serviceId}`, { method: 'POST', body: { labels: { 'momso-install-id': id }, invokerIamDisabled: false, template: template('pending.invalid') } });
      await this.#pollRun(projectId, operation.name);
      service = await this.#request(endpoint);
    }
    if (service.labels?.['momso-install-id'] !== id) throw new SetupError('gateway_ownership');
    const gatewayPolicy = await this.#request(`${endpoint}:getIamPolicy?options.requestedPolicyVersion=3`);
    if ((gatewayPolicy.bindings ?? []).some(item => (item.members ?? []).some(member => member === 'allUsers' || member === 'allAuthenticatedUsers'))) throw new SetupError('gateway_policy');
    let uri;
    try {
      const parsed = new URL(service.uri);
      if (parsed.protocol !== 'https:' || parsed.origin !== service.uri || !parsed.hostname.endsWith('.run.app')) throw new Error();
      uri = parsed;
    } catch { throw new SetupError('gateway_address'); }
    await activatePinnedRevision({
      image: this.#release.gatewayImage, service, template: template(uri.hostname),
      read: () => this.#request(endpoint),
      patch: async (body, mask) => {
        const operation = await this.#request(`${endpoint}?updateMask=${mask}`, { method: 'PATCH', body: { name: serviceName, ...body } });
        await this.#pollRun(projectId, operation.name);
      },
      revision: name => this.#request(`${endpoint}/revisions/${name}`),
    });
    this.#gatewayUrl = uri.origin;
  }

  async gatewayReady(id) {
    const projectId = this.#projectId(id);
    const serviceId = `momso-any-${id.slice(0, 12)}`;
    const service = await this.#request(`${RUN}projects/${projectId}/locations/${REGION}/services/${serviceId}`);
    if (service.labels?.['momso-install-id'] !== id || service.reconciling || service.terminalCondition?.state !== 'CONDITION_SUCCEEDED' || service.uri !== this.#gatewayUrl || !service.invokerIamDisabled) throw new SetupError('gateway_not_ready');
  }

  async destroy(id, onProgress = () => {}) {
    if (!this.#selectedProject) throw new SetupError('project_required');
    const projectId = this.#projectId(id);
    const short = id.slice(0, 12);
    const name = `momso-${short}`;
    const marker = `momso-install-${id}`;
    const base = `${COMPUTE}projects/${projectId}/`;
    const runBase = `${RUN}projects/${projectId}/locations/${REGION}/services/momso-any-${short}`;
    const items = [
      { key: 'gateway', url: runBase, type: 'run', owns: x => x.labels?.['momso-install-id'] === id && x.name === `projects/${projectId}/locations/${REGION}/services/momso-any-${short}` },
      { key: 'vm', url: `${base}zones/${ZONE}/instances/${name}`, type: 'compute', owns: x => x.labels?.['momso-install-id'] === id && x.name === name && x.disks?.length === 1 && x.disks[0].boot === true && x.disks[0].autoDelete === true && x.disks[0].source?.endsWith('/disks/' + name) && x.serviceAccounts?.length === 1 && x.serviceAccounts[0].email === `momso-vm-${short}@${projectId}.iam.gserviceaccount.com` },
      { key: 'router', url: `${base}regions/${REGION}/routers/${name}-router`, type: 'compute', owns: x => x.name === `${name}-router` && x.description === marker && (x.nats ?? []).every(nat => nat.name === `${name}-nat`) && !(x.bgpPeers?.length) && !(x.interfaces?.length) },
      { key: 'firewall', url: `${base}global/firewalls/${name}-private`, type: 'compute', owns: x => x.name === `${name}-private` && x.description === marker && x.network?.endsWith('/' + name + '-net') },
      { key: 'subnet', url: `${base}regions/${REGION}/subnetworks/${name}-sub`, type: 'compute', owns: x => x.name === `${name}-sub` && x.description === marker && x.network?.endsWith('/' + name + '-net') },
      { key: 'network', url: `${base}global/networks/${name}-net`, type: 'compute', owns: x => x.name === `${name}-net` && x.description === marker },
      ...['transport', 'session', 'vault'].map(suffix => ({ key: `secret_${suffix}`, url: `${SECRETS}projects/${projectId}/secrets/momso-${suffix}-${short}`, type: 'secret', owns: x => x.labels?.['momso-install-id'] === id && x.name?.endsWith(`/secrets/momso-${suffix}-${short}`) })),
      ...['vm', 'gateway'].map(suffix => ({ key: `account_${suffix}`, url: `${IAM}projects/${projectId}/serviceAccounts/momso-${suffix}-${short}@${projectId}.iam.gserviceaccount.com`, type: 'account', owns: x => x.email === `momso-${suffix}-${short}@${projectId}.iam.gserviceaccount.com` && x.description === marker })),
      ...[['Stop', ['compute.instances.stop']], ['Anywhere', ['compute.instances.get', 'compute.instances.start']]].map(([suffix, permissions]) => ({ key: `role_${suffix.toLowerCase()}`, url: `${IAM}projects/${projectId}/roles/momso${suffix}${short}`, type: 'role', owns: x => x.name?.endsWith(`/roles/momso${suffix}${short}`) && x.description === marker && JSON.stringify([...(x.includedPermissions ?? [])].sort()) === JSON.stringify(permissions) })),
    ];
    await this.project(id);
    for (const item of items) {
      item.before = await this.#request(item.url, { allowMissing: true });
      if (item.type === 'role' && item.before?.deleted) item.before = null;
      if (item.before && !item.owns(item.before)) throw new SetupError('resource_ownership');
    }
    const remaining = [];
    let networkPending = false;
    for (const item of items) {
      if (!item.before) continue;
      if (networkPending && item.key === 'network') { remaining.push(item.key); continue; }
      const current = await this.#request(item.url, { allowMissing: true });
      if (!current) continue;
      if (!item.owns(current) || (item.before.id && current.id !== item.before.id) || (item.before.uniqueId && current.uniqueId !== item.before.uniqueId) || (item.before.etag && current.etag !== item.before.etag)) throw new SetupError('resource_ownership');
      onProgress({ step: item.key, state: 'working' });
      try {
        if (item.key === 'router' && current.nats?.length) {
          const natRemoval = await this.#request(item.url, { method: 'PATCH', body: { nats: [] } });
          await this.#computeOperation(projectId, natRemoval);
          const refreshed = await this.#request(item.url, { allowMissing: true });
          if (!refreshed || !item.owns(refreshed) || refreshed.nats?.length || (current.id && refreshed.id !== current.id)) throw new SetupError('resource_ownership');
        }
        const etag = current.etag && (item.type === 'run' || item.type === 'secret') ? `?etag=${encodeURIComponent(current.etag)}` : '';
        const op = await this.#request(item.url + etag, { method: 'DELETE' });
        if (item.type === 'run') await this.#pollRun(projectId, op.name);
        if (item.type === 'compute') await this.#computeOperation(projectId, op);
        onProgress({ step: item.key, state: 'done' });
      } catch (error) {
        if ((item.key === 'subnet' || item.key === 'network') && error instanceof SetupError && (error.step === 'google_request' || error.step === 'operation_failed')) {
          remaining.push(item.key); networkPending = true; onProgress({ step: item.key, state: 'pending' });
        } else throw error;
      }
    }
    return { remaining };
  }
}
