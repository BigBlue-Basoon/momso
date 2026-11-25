import { SetupError, bindCustomRole } from './momso_anywhere_setup.mjs';
import { vmStartupScript } from './momso_anywhere_vm_startup.mjs';
import { prepareBackupResources } from './momso_anywhere_backup_setup.mjs';
import { activatePinnedRevision } from './momso_anywhere_cloud_run_revision.mjs';
import { verifyRelease } from './momso_release_manifest.mjs';
import { deletionPreview } from './momso_anywhere_deletion_contract.mjs';
import { discardBackup } from './momso_anywhere_discard_backup.mjs';
import { inspectProjectExclusivity, INSTALLER_SERVICES } from './momso_anywhere_project_exclusivity.mjs';
import { removeAndVerify } from './momso_anywhere_iam_policy.mjs';
import { restorePlan } from './momso_anywhere_restore_plan.mjs';

const CRM = 'https://cloudresourcemanager.googleapis.com/v3/';
const BILLING = 'https://cloudbilling.googleapis.com/v1/';
const USAGE = 'https://serviceusage.googleapis.com/v1/';
const COMPUTE = 'https://compute.googleapis.com/compute/v1/';
const IAM = 'https://iam.googleapis.com/v1/';
const SECRETS = 'https://secretmanager.googleapis.com/v1/';
const RUN = 'https://run.googleapis.com/v2/';
const ASSET = 'https://cloudasset.googleapis.com/v1/';
const REGION = 'asia-northeast3';
const ZONE = 'asia-northeast3-a';
const SERVICES = INSTALLER_SERVICES;

export async function providerFromToken({ token, expiresAt, clientId, release, billingAccountName = null, projectId = null, fetchImpl = fetch, trustStorage = globalThis.localStorage }) {
  if (!token || !Number.isFinite(expiresAt) || expiresAt <= Date.now()) throw new SetupError('authorization_expired');
  let response;
  try { response = await fetchImpl('https://openidconnect.googleapis.com/v1/userinfo', { headers: { Authorization: `Bearer ${token}` }, cache: 'no-store', redirect: 'error' }); }
  catch { throw new SetupError('identity_unavailable'); }
  if (!response.ok) throw new SetupError(response.status === 401 ? 'authorization_expired' : 'identity_unavailable');
  const identity = await response.json();
  if (!/^[0-9]{1,40}$/.test(identity.sub ?? '')) throw new SetupError('identity_unavailable');
  return { ownerSub: identity.sub, provider: new GoogleBrowserProvider(token, { fetchImpl, release, clientId, ownerSub: identity.sub, billingAccountName, projectId, expiresAt, trustStorage }) };
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
  #trustStorage;
  #restorePlan = null;
  #restoreResult = null;
  constructor(accessToken, { fetchImpl = fetch, sleep = ms => new Promise(resolve => setTimeout(resolve, ms)), release = null, clientId = null, ownerSub = null, billingAccountName = null, projectId = null, expiresAt = Infinity, releaseVerifier = verifyRelease, trustStorage = globalThis.localStorage } = {}) {
    if (!accessToken) throw new SetupError('invalid_authorization');
    this.#token = accessToken;
    this.#fetch = fetchImpl;
    this.#sleep = sleep;
    this.#release = null;
    this.#releaseConfig = release;
    this.#releaseVerifier = releaseVerifier;
    this.#trustStorage = trustStorage;
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
    if (![CRM, BILLING, USAGE, COMPUTE, IAM, SECRETS, RUN, ASSET, 'https://storage.googleapis.com/storage/v1/', 'https://cloudkms.googleapis.com/v1/'].some(base => url.startsWith(base))) throw new SetupError('invalid_api');
    let response;
    try {
      response = await this.#fetch(url, { method, headers: { Authorization: `Bearer ${this.#token}`, ...(body ? { 'Content-Type': 'application/json' } : {}) }, ...(body ? { body: JSON.stringify(body) } : {}), cache: 'no-store', redirect: 'error' });
    } catch { throw new SetupError('network'); }
    if ((allowMissing && response.status === 404) || (missingForbidden && response.status === 403)) return null;
    if (!response.ok) {
      if (response.status === 403) {
        let message = '';
        try { message = (await response.clone().json())?.error?.message ?? ''; } catch { /* no verified image permission reason */ }
        if (typeof message === 'string' && /artifactregistry\.repositories\.downloadArtifacts|permission to read the image/i.test(message)) throw new SetupError('gateway_image_permission');
      }
      if (response.status === 400) {
        let message = '';
        try { message = (await response.json())?.error?.message ?? ''; } catch { /* preserve generic error */ }
        if (typeof message === 'string' && /role\s*\([^\r\n]*?\/roles\/[^\r\n)]*\)\s*does not exist in the resource.s hierarchy/i.test(message)) {
          throw new SetupError('role_propagating');
        }
      }
      const reason = response.status === 401 ? 'authorization_expired' : response.status === 403 ? 'permission_denied' : response.status === 402 ? 'billing_setup_required' : response.status === 429 ? 'quota_exceeded' : response.status >= 500 ? 'google_unavailable' : 'google_request';
      const error = new SetupError(reason);
      if (response.status === 403) error.checks = ['billing_account', 'terms', 'project_permissions'];
      error.requestRejected = response.status >= 400 && response.status < 500 && ![408, 429].includes(response.status);
      throw error;
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
    if (!/^[a-f0-9]{12}(?:[a-f0-9]{12})?$/.test(id)) throw new SetupError('invalid_checkpoint');
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
      if (!this.#release) this.#release = await this.#releaseVerifier(this.#releaseConfig, { fetchImpl: this.#fetch, storage: this.#trustStorage });
      vmStartupScript({ project: 'fixture-project', secret: 'momso-transport-aaaaaaaaaaaa', bundleUrl: this.#release.bundleUrl, bundleSha256: this.#release.bundleSha256 });
      const origin = new URL(this.#release.loginOrigin);
      if (origin.protocol !== 'https:' || origin.origin !== this.#release.loginOrigin || !/^[a-z0-9.-]+\/[a-z0-9._/-]+@sha256:[a-f0-9]{64}$/.test(this.#release.gatewayImage) || !/^[A-Za-z0-9._-]+\.apps\.googleusercontent\.com$/.test(this.#clientId ?? '') || !/^[0-9]{1,40}$/.test(this.#ownerSub ?? '')) throw new Error();
    } catch { throw new SetupError('release_not_ready'); }
  }

  gatewayUrl() { return this.#gatewayUrl ?? null; }

  async pauseGateway(id) {
    if (!this.#selectedProject) throw new SetupError('project_required');
    const projectId = this.#projectId(id);
    const service = await this.#request(`${RUN}projects/${projectId}/locations/${REGION}/services/momso-any-${id.slice(0, 12)}`);
    const env = Object.fromEntries((service.template?.containers?.[0]?.env ?? []).filter(item => typeof item.value === 'string').map(item => [item.name, item.value]));
    let uri;
    try { uri = new URL(service.uri); } catch { throw new SetupError('resource_ownership'); }
    if (service.name !== `projects/${projectId}/locations/${REGION}/services/momso-any-${id.slice(0, 12)}` ||
        service.labels?.['momso-install-id'] !== id || uri.protocol !== 'https:' || uri.origin !== service.uri ||
        !uri.hostname.endsWith('.run.app') || env.MOMSO_PUBLIC_HOST !== uri.hostname ||
        env.MOMSO_OWNER_SUB !== this.#ownerSub || env.MOMSO_GOOGLE_CLIENT_ID !== this.#clientId ||
        env.MOMSO_PROJECT !== projectId || env.MOMSO_INSTANCE !== `momso-${id.slice(0, 12)}`) throw new SetupError('resource_ownership');
    return uri.origin;
  }

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
        if (!/^[a-f0-9]{12}(?:[a-f0-9]{12})?$/.test(id ?? '') || service.name !== `projects/${project.projectId}/locations/${REGION}/services/momso-any-${id.slice(0, 12)}`) continue;
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

  async restorePlan(id) {
    if (!/^[a-f0-9]{12}(?:[a-f0-9]{12})?$/.test(id ?? '')) throw new SetupError('invalid_installation');
    const project = this.#projectId(id), short = id.slice(0, 12);
    const instance = await this.#request(`${COMPUTE}projects/${project}/zones/${ZONE}/instances/momso-${short}`, { allowMissing: true });
    if (instance) {
      if (instance.labels?.['momso-install-id'] !== id) throw new SetupError('resource_ownership');
      return { restoreAvailable: null, lastBackupAt: null, requiresRestore: false, existingVm: true };
    }
    const instances = await this.#projectInstances(project);
    if (instances.some(item => item.labels?.['momso-install-id'] === id)) throw new SetupError('restore_vm_exists');
    const secretUrl = `${SECRETS}projects/${project}/secrets/momso-vault-${short}`;
    const bucketUrl = `https://storage.googleapis.com/storage/v1/b/momso-backup-${short}`;
    const keyUrl = `https://cloudkms.googleapis.com/v1/projects/${project}/locations/${REGION}/keyRings/momso-backup-${short}/cryptoKeys/backup`;
    const [secret, bucket, key] = await Promise.all([secretUrl, bucketUrl, keyUrl].map(url => this.#request(url, { allowMissing: true })));
    for (const item of [secret, bucket, key]) if (item && item.labels?.['momso-install-id'] !== id) throw new SetupError('resource_ownership');
    if (![secret, bucket, key].some(Boolean)) return restorePlan({ vault: 'absent', bucket: 'absent', key: 'absent', slots: null });
    if (![secret, bucket, key].every(Boolean)) throw new SetupError('restore_assets_incomplete');
    const [secretVersions, keyVersions] = await Promise.all([this.#request(secretUrl + '/versions?pageSize=100'), this.#request(keyUrl + '/cryptoKeyVersions?pageSize=100')]);
    if (secretVersions.nextPageToken || keyVersions.nextPageToken || !Array.isArray(secretVersions.versions) || !Array.isArray(keyVersions.cryptoKeyVersions)) throw new SetupError('restore_assets_unverified');
    if (!secretVersions.versions.some(item => item.state === 'ENABLED' && item.name?.endsWith('/versions/1')) || !keyVersions.cryptoKeyVersions.some(item => item.state === 'ENABLED')) throw new SetupError('restore_assets_incomplete');
    const slots = [], seen = new Set(); let page = '';
    do {
      if (seen.has(page)) throw new SetupError('restore_assets_unverified');
      seen.add(page);
      const query = new URLSearchParams({ prefix: 'momso-backup/', maxResults: '100' });
      if (page) query.set('pageToken', page);
      const result = await this.#request(bucketUrl + '/o?' + query);
      if (result.items !== undefined && !Array.isArray(result.items)) throw new SetupError('restore_assets_unverified');
      slots.push(...(result.items ?? []));
      page = result.nextPageToken ?? '';
    } while (page);
    return restorePlan({ vault: 'ready', bucket: 'ready', key: 'ready', slots });
  }

  enableRestore(id, plan) {
    if (!/^[a-f0-9]{12}(?:[a-f0-9]{12})?$/.test(id) || plan?.requiresRestore !== true) throw new SetupError('restore_assets_unverified');
    this.#restorePlan = { id, plan };
  }

  restoreResult() { return this.#restoreResult ?? {}; }

  async #projectInstances(project) {
    const instances = [], seen = new Set(); let page = '';
    do {
      if (seen.has(page)) throw new SetupError('restore_assets_unverified');
      seen.add(page);
      const query = new URLSearchParams({ maxResults: '500' });
      if (page) query.set('pageToken', page);
      const result = await this.#request(`${COMPUTE}projects/${project}/aggregated/instances?${query}`);
      if (!result.items || typeof result.items !== 'object' || Array.isArray(result.items)) throw new SetupError('restore_assets_unverified');
      for (const scope of Object.values(result.items)) {
        if (!scope || typeof scope !== 'object' || (scope.warning && scope.warning.code !== 'NO_RESULTS_ON_PAGE')) throw new SetupError('restore_assets_unverified');
        if (scope.instances !== undefined && !Array.isArray(scope.instances)) throw new SetupError('restore_assets_unverified');
        instances.push(...(scope.instances ?? []));
      }
      page = result.nextPageToken ?? '';
    } while (page);
    return instances;
  }

  async discoverPaused() {
    if (!this.#selectedProject) throw new SetupError('project_required');
    const project = this.#selectedProject;
    const projectInfo = await this.#request(`${CRM}projects/${project}`);
    const projectNumber = /^projects\/(\d+)$/.exec(projectInfo.name ?? '')?.[1];
    if (!projectNumber) throw new SetupError('restore_assets_unverified');
    const list = async (url, field) => {
      const items = [], seen = new Set(); let page = '';
      do {
        if (seen.has(page)) throw new SetupError('restore_assets_unverified');
        seen.add(page);
        const target = new URL(url);
        target.searchParams.set(target.hostname === 'storage.googleapis.com' ? 'maxResults' : 'pageSize', '100');
        if (page) target.searchParams.set('pageToken', page);
        const result = await this.#request(target.href);
        if (result[field] !== undefined && !Array.isArray(result[field])) throw new SetupError('restore_assets_unverified');
        items.push(...(result[field] ?? []));
        page = result.nextPageToken ?? '';
      } while (page);
      return items;
    };
    const [buckets, secrets, rings, instances] = await Promise.all([
      list(`https://storage.googleapis.com/storage/v1/b?project=${project}`, 'items'),
      list(`${SECRETS}projects/${project}/secrets`, 'secrets'),
      list(`https://cloudkms.googleapis.com/v1/projects/${project}/locations/${REGION}/keyRings`, 'keyRings'),
      this.#projectInstances(project),
    ]);
    const keys = [];
    for (const ring of rings) {
      if (!new RegExp(`^projects/(?:${project}|${projectNumber})/locations/${REGION}/keyRings/momso-backup-[a-f0-9]{12}$`).test(ring.name ?? '')) continue;
      keys.push(...await list(`https://cloudkms.googleapis.com/v1/${ring.name}/cryptoKeys`, 'cryptoKeys'));
    }
    const found = new Map();
    for (const [kind, items] of [['bucket', buckets], ['vault', secrets], ['key', keys]]) for (const item of items) {
      const id = item?.labels?.['momso-install-id'];
      if (!/^(?:[a-f0-9]{12}|[a-f0-9]{24})$/.test(id ?? '')) continue;
      const short = id.slice(0, 12), name = item.name ?? '';
      const expected = kind === 'bucket' ? [`momso-backup-${short}`] : kind === 'vault' ? [project, projectNumber].map(p => `projects/${p}/secrets/momso-vault-${short}`) : [project, projectNumber].map(p => `projects/${p}/locations/${REGION}/keyRings/momso-backup-${short}/cryptoKeys/backup`);
      if (!expected.includes(name)) continue;
      if (!found.has(id)) found.set(id, new Set());
      found.get(id).add(kind);
    }
    const result = [];
    for (const [id, kinds] of found) {
      if (instances.some(item => item.labels?.['momso-install-id'] === id)) continue;
      const instance = instances.find(item => item.name === `momso-${id.slice(0, 12)}`);
      if (instance && instance.labels?.['momso-install-id'] !== id) throw new SetupError('resource_ownership');
      if (!instance) {
        const candidate = { installationId: id, assets: kinds.size === 3 ? 'complete' : 'incomplete', browserIdCompatible: true };
        if (candidate.assets === 'complete') Object.assign(candidate, await this.restorePlan(id));
        else Object.assign(candidate, { restoreAvailable: null, lastBackupAt: null });
        result.push(candidate);
      }
    }
    return result.sort((a, b) => a.installationId.localeCompare(b.installationId));
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
    if (!selected) throw new SetupError(this.#billingAccountName ? 'billing_account_unavailable' : open.length ? 'billing_selection_required' : 'billing_account_missing');
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

  async preparePrivateIdentity(id, { preservedOnly = false } = {}) {
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
        if (preservedOnly && suffix === 'vault') throw new SetupError('restore_assets_incomplete');
        await this.#request(`${SECRETS}projects/${projectId}/secrets?secretId=${secret}`, { method: 'POST', body: { replication: { automatic: {} }, labels: { 'momso-install-id': id } } });
        item = await this.#request(base);
      }
      if (item.labels?.['momso-install-id'] !== id) throw new SetupError('secret_ownership');
      const versions = await this.#request(`${base}/versions`);
      if (versions.nextPageToken) throw new SetupError('secret_versions');
      if (preservedOnly && suffix === 'vault' && !(versions.versions ?? []).some(version => version.state === 'ENABLED' && version.name?.endsWith('/versions/1'))) throw new SetupError('restore_assets_incomplete');
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
      await this.#retryPropagation(() => bindCustomRole(() => this.#request(`${base}:setIamPolicy`, { method: 'POST', body: { policy: { ...policy, version: 3, bindings } } }), this.#sleep));
    }
  }

  async server(id) {
    await this.preflight();
    const projectId = this.#projectId(id);
    if (!this.#release) throw new SetupError('release_not_ready');
    const short = id.slice(0, 12);
    const name = `momso-${short}`;
    const account = `momso-vm-${short}@${projectId}.iam.gserviceaccount.com`;
    const update = this.#releaseConfig?.manifestUrl && this.#releaseConfig?.publicKeySpki
      ? { manifestUrl: this.#releaseConfig.manifestUrl, publicKeySpki: this.#releaseConfig.publicKeySpki, channel: this.#releaseConfig.channel ?? 'production', ...(this.#releaseConfig.channel === 'development' ? {} : { trustBootstrap: this.#releaseConfig.trustBootstrap }) }
      : null;
    const path = `zones/${ZONE}/instances/${name}`;
    const prior = await this.#request(`${COMPUTE}projects/${projectId}/${path}`, { allowMissing: true });
    if (prior && prior.labels?.['momso-install-id'] !== id) throw new SetupError('resource_ownership');
    if (!prior && this.#restorePlan?.id !== id) {
      const plan = await this.restorePlan(id);
      if (plan.requiresRestore) this.enableRestore(id, plan);
    }
    let restoring = this.#restorePlan?.id === id;
    const startup = restore => vmStartupScript({ project: projectId, secret: `momso-transport-${short}`, bundleUrl: this.#release.bundleUrl, bundleSha256: this.#release.bundleSha256, update, restore: restore ? { installationId: id } : null });
    if (prior) {
      const existing = prior.metadata?.items?.find(item => item.key === 'startup-script')?.value;
      if (existing === startup(true)) restoring = true;
      else if (existing === startup(false)) restoring = false;
      else throw new SetupError('resource_ownership');
    }
    const script = startup(restoring);
    let identityPrepared = false;
    try {
    await this.preparePrivateIdentity(id, { preservedOnly: restoring });
    identityPrepared = true;
    await prepareBackupResources((url, options) => this.#request(url, options), projectId, id, this.#sleep, { preservedOnly: restoring });
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
        if (restoring) {
          const restoreAttrs = await this.#request(`${COMPUTE}projects/${projectId}/${path}/getGuestAttributes?queryPath=momso%2Frestore`);
          const value = restoreAttrs.queryValue?.items?.find(item => item.namespace === 'momso' && item.key === 'restore')?.value;
          if (value) {
            let result;
            try { result = JSON.parse(value); } catch { throw new SetupError('restore_failed'); }
            if (result.ok === false) { const error = new SetupError('restore_failed'); error.detail = result.detail; throw error; }
            if (typeof result.restored !== 'boolean') throw new SetupError('restore_failed');
            this.#restoreResult = { restored: result.restored, restoredBackupId: result.restoredBackupId ?? null, restoredAt: result.restoredAt ?? null };
          }
        }
        const attrs = await this.#request(`${COMPUTE}projects/${projectId}/${path}/getGuestAttributes?queryPath=momso%2Ffingerprint`);
        const fingerprint = attrs.queryValue?.items?.find(item => item.namespace === 'momso' && item.key === 'fingerprint')?.value;
        if (/^[a-f0-9]{64}$/.test(fingerprint ?? '') && (!restoring || this.#restoreResult)) {
          await this.#revokeVaultAdder(projectId, short, account, true);
          identityPrepared = false;
          return;
        }
      } catch (error) {
        if (error instanceof SetupError && error.step === 'restore_failed') throw error;
        if (!(error instanceof SetupError)) throw error;
      }
      await this.#sleep(1000);
    }
    if (restoring) { const error = new SetupError('restore_failed'); error.detail = 'backup_unreadable'; throw error; }
    throw new SetupError('server_not_ready');
    } finally {
      if (identityPrepared) await this.#revokeVaultAdder(projectId, short, account, false);
    }
  }

  async #reconcileRole(projectId, roleId, marker, permissions) {
    const path = `${IAM}projects/${projectId}/roles/${roleId}`;
    let role = await this.#request(path, { allowMissing: true });
    if (!role) {
      await this.#request(`${IAM}projects/${projectId}/roles`, { method: 'POST', body: { roleId, role: { title: roleId, description: marker, includedPermissions: permissions, stage: 'GA' } } });
      role = await this.#request(path);
    } else {
      if (role.description !== marker) throw new SetupError('role_ownership');
      if (role.deleted) {
        await this.#request(`${path}:undelete`, { method: 'POST', body: {} });
        role = await this.#request(path);
      }
      if (JSON.stringify([...(role.includedPermissions ?? [])].sort()) !== JSON.stringify([...permissions].sort())) {
        await this.#request(`${path}?updateMask=includedPermissions`, { method: 'PATCH', body: { includedPermissions: permissions } });
        role = await this.#request(path);
      }
    }
    if (role.description !== marker || role.deleted || JSON.stringify([...(role.includedPermissions ?? [])].sort()) !== JSON.stringify([...permissions].sort())) throw new SetupError('role_ownership');
    return role;
  }

  async #grantVmStop(projectId, id, instance, account) {
    const roleId = `momsoStop${id.slice(0, 12)}`;
    const marker = `momso-install-${id}`;
    await this.#reconcileRole(projectId, roleId, marker, ['compute.instances.stop']);
    const endpoint = `${COMPUTE}projects/${projectId}/zones/${ZONE}/instances/${instance}`;
    const policy = await this.#request(`${endpoint}/getIamPolicy?optionsRequestedPolicyVersion=3`);
    if (!policy.etag) throw new SetupError('policy_unavailable');
    const bindings = policy.bindings ?? [];
    const roleName = `projects/${projectId}/roles/${roleId}`;
    let binding = bindings.find(value => value.role === roleName && !value.condition);
    if (!binding) { binding = { role: roleName, members: [] }; bindings.push(binding); }
    binding.members = [...new Set([...(binding.members ?? []), `serviceAccount:${account}`])];
    await bindCustomRole(() => this.#request(`${endpoint}/setIamPolicy`, { method: 'POST', body: { policy: { ...policy, version: 3, bindings } } }), this.#sleep);
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
    await bindCustomRole(() => this.#request(`${base}:setIamPolicy`, { method: 'POST', body: { policy: { ...policy, version: 3, bindings } } }), this.#sleep);
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
    await this.#reconcileRole(projectId, roleId, marker, ['compute.instances.get', 'compute.instances.start']);
    const policy = await this.#request(`${instance}/getIamPolicy?optionsRequestedPolicyVersion=3`);
    if (!policy.etag) throw new SetupError('policy_unavailable');
    const roleName = `projects/${projectId}/roles/${roleId}`;
    const bindings = policy.bindings ?? [];
    let binding = bindings.find(item => item.role === roleName && !item.condition);
    if (!binding) { binding = { role: roleName, members: [] }; bindings.push(binding); }
    binding.members = [...new Set([...(binding.members ?? []), `serviceAccount:${account}`])];
    await bindCustomRole(() => this.#request(`${instance}/setIamPolicy`, { method: 'POST', body: { policy: { ...policy, version: 3, bindings } } }), this.#sleep);
    const serviceId = `momso-any-${short}`;
    const serviceName = `projects/${projectId}/locations/${REGION}/services/${serviceId}`;
    const endpoint = RUN + serviceName;
    const env = host => Object.entries({ MOMSO_PRIVATE_HOST: '10.42.0.10', MOMSO_PRIVATE_PORT: '8766', MOMSO_PRIVATE_CERT_SHA256: pin, MOMSO_PROJECT: projectId, MOMSO_ZONE: ZONE, MOMSO_INSTANCE: name, MOMSO_GOOGLE_CLIENT_ID: this.#clientId, MOMSO_OWNER_SUB: this.#ownerSub, MOMSO_LOGIN_ORIGIN: this.#release.loginOrigin, MOMSO_SESSION_KEY_FILE: '/secrets/session-key', MOMSO_PUBLIC_HOST: host }).map(([key, value]) => ({ name: key, value }));
    const template = host => ({ serviceAccount: account, timeout: '3600s', vpcAccess: { networkInterfaces: [{ network: `${name}-net`, subnetwork: `${name}-sub` }], egress: 'PRIVATE_RANGES_ONLY' }, volumes: [{ name: 'session', secret: { secret: `momso-session-${short}`, items: [{ version: 'latest', path: 'session-key' }] } }], containers: [{ image: this.#release.gatewayImage, env: [...env(host), { name: 'MOMSO_PRIVATE_PROXY_TOKEN', valueSource: { secretKeyRef: { secret: `momso-transport-${short}`, version: 'latest' } } }], volumeMounts: [{ name: 'session', mountPath: '/secrets' }] }] });
    let service = await this.#request(endpoint, { allowMissing: true });
    let created = false;
    if (service && service.labels?.['momso-install-id'] !== id) throw new SetupError('gateway_ownership');
    const previousPin = service?.template?.containers?.[0]?.env?.find(item => item.name === 'MOMSO_PRIVATE_CERT_SHA256')?.value;
    if (previousPin && previousPin !== pin) throw new SetupError('certificate_changed');
    if (!service) {
      const operation = await this.#request(`${RUN}projects/${projectId}/locations/${REGION}/services?serviceId=${serviceId}`, { method: 'POST', body: { labels: { 'momso-install-id': id }, scaling: { maxInstanceCount: 1 }, invokerIamDisabled: false, template: template('pending.invalid') } });
      created = true;
      await this.#pollRun(projectId, operation.name);
      service = await this.#request(endpoint);
    }
    if (service.labels?.['momso-install-id'] !== id) throw new SetupError('gateway_ownership');
    const gatewayPolicy = await this.#request(`${endpoint}:getIamPolicy?options.requestedPolicyVersion=3`);
    if ((gatewayPolicy.bindings ?? []).some(item => (item.members ?? []).some(member => member === 'allUsers' || member === 'allAuthenticatedUsers'))) throw new SetupError('gateway_policy');
    const updateRoleId = `momsoUpdate${short}`;
    const updatePermissions = ['run.operations.get', 'run.revisions.get', 'run.revisions.list', 'run.services.get', 'run.services.update'];
    await this.#reconcileRole(projectId, updateRoleId, marker, updatePermissions);
    if (!gatewayPolicy.etag) throw new SetupError('policy_unavailable');
    const vmMember = `serviceAccount:momso-vm-${short}@${projectId}.iam.gserviceaccount.com`;
    const gatewayBindings = gatewayPolicy.bindings ?? [];
    const updateRoleName = `projects/${projectId}/roles/${updateRoleId}`;
    let updateBinding = gatewayBindings.find(item => item.role === updateRoleName && !item.condition);
    if (!updateBinding) { updateBinding = { role: updateRoleName, members: [] }; gatewayBindings.push(updateBinding); }
    updateBinding.members = [...new Set([...(updateBinding.members ?? []), vmMember])];
    await bindCustomRole(() => this.#request(`${endpoint}:setIamPolicy`, { method: 'POST', body: { policy: { ...gatewayPolicy, version: 3, bindings: gatewayBindings } } }), this.#sleep);
    const accountPath = `${IAM}projects/${projectId}/serviceAccounts/${account}`;
    const accountPolicy = await this.#request(`${accountPath}:getIamPolicy?options.requestedPolicyVersion=3`);
    if (!accountPolicy.etag) throw new SetupError('policy_unavailable');
    const accountBindings = accountPolicy.bindings ?? [];
    let accountBinding = accountBindings.find(item => item.role === 'roles/iam.serviceAccountUser' && !item.condition);
    if (!accountBinding) { accountBinding = { role: 'roles/iam.serviceAccountUser', members: [] }; accountBindings.push(accountBinding); }
    accountBinding.members = [...new Set([...(accountBinding.members ?? []), vmMember])];
    await this.#request(`${accountPath}:setIamPolicy`, { method: 'POST', body: { policy: { ...accountPolicy, version: 3, bindings: accountBindings } } });
    let uri;
    try {
      const parsed = new URL(service.uri);
      if (parsed.protocol !== 'https:' || parsed.origin !== service.uri || !parsed.hostname.endsWith('.run.app')) throw new Error();
      uri = parsed;
    } catch { throw new SetupError('gateway_address'); }
    try { await activatePinnedRevision({
      image: this.#release.gatewayImage, service, template: template(uri.hostname),
      read: () => this.#request(endpoint),
      patch: async (body, mask) => {
        const operation = await this.#request(`${endpoint}?updateMask=${mask}`, { method: 'PATCH', body: { name: serviceName, ...body } });
        await this.#pollRun(projectId, operation.name);
      },
      revision: name => this.#request(`${endpoint}/revisions/${name}`),
      probe: async () => {
        for (let attempt = 0; attempt < 3; attempt++) {
          try {
            const reply = await this.#fetch(uri.origin + '/__momso/ready', { cache: 'no-store', credentials: 'omit', redirect: 'error', signal: AbortSignal.timeout(20000) });
            const body = await reply.json();
            if (reply.ok && body?.ready === true && Object.keys(body).length === 1) {
              await this.#probeProxy(id, uri.origin);
              return true;
            }
          } catch (error) { if (error?.step === 'proxy_not_running') throw error; }
          if (attempt < 2) await this.#sleep(2000);
        }
        return false;
      },
    }); } catch (error) {
      if (created) {
        try {
          const owned = await this.#request(endpoint, { allowMissing: true });
          if (owned && owned.labels?.['momso-install-id'] !== id) throw new Error();
          if (owned) {
            const operation = await this.#request(endpoint, { method: 'DELETE' });
            await this.#pollRun(projectId, operation.name);
          }
        } catch { throw Object.assign(new SetupError('gateway_rollback_failed'), { code: 'gateway_rollback_failed', rolledBack: false, retryable: false }); }
      }
      throw error;
    }
    this.#gatewayUrl = uri.origin;
  }

  async gatewayReady(id) {
    const projectId = this.#projectId(id);
    const serviceId = `momso-any-${id.slice(0, 12)}`;
    const service = await this.#request(`${RUN}projects/${projectId}/locations/${REGION}/services/${serviceId}`);
    if (service.labels?.['momso-install-id'] !== id || service.reconciling || service.terminalCondition?.state !== 'CONDITION_SUCCEEDED' || service.uri !== this.#gatewayUrl || !service.invokerIamDisabled) throw new SetupError('gateway_not_ready');
  }

  async #probeProxy(id, gatewayUrl) {
    const projectId = this.#projectId(id);
    const secretUrl = `${SECRETS}projects/${projectId}/secrets/momso-transport-${id.slice(0, 12)}`;
    const secret = await this.#request(secretUrl);
    if (secret.labels?.['momso-install-id'] !== id) throw new SetupError('resource_ownership');
    const version = await this.#request(secretUrl + '/versions/latest:access');
    const encoded = version.payload?.data;
    if (typeof encoded !== 'string' || !/^[A-Za-z0-9+/]+={0,2}$/.test(encoded) || encoded.length > 512) throw new SetupError('proxy_not_running');
    let token, proof;
    try {
      token = Uint8Array.from(atob(encoded), char => char.charCodeAt(0));
      if (token.length < 32 || token.length > 256) throw new Error();
      const key = await crypto.subtle.importKey('raw', token, { name: 'HMAC', hash: 'SHA-256' }, false, ['sign']);
      proof = [...new Uint8Array(await crypto.subtle.sign('HMAC', key, new TextEncoder().encode('momso-proxy-health-v1')))].map(value => value.toString(16).padStart(2, '0')).join('');
    } catch { throw new SetupError('proxy_not_running'); }
    finally { token?.fill(0); }
    for (let attempt = 0; attempt < 30; attempt++) {
      try {
        const response = await this.#fetch(`${gatewayUrl}/__momso/proxy-health`, { method: 'GET', headers: { 'X-Momso-Health-Proof': proof }, credentials: 'omit', cache: 'no-store', redirect: 'error', signal: AbortSignal.timeout(10000) });
        if (response.ok && (await response.json())?.state === 'ready') return;
      } catch { /* retry without exposing the proof */ }
      if (attempt < 29) await this.#sleep(1000);
    }
    throw new SetupError('proxy_not_running');
  }

  async deletionPreview(id) {
    if (!this.#selectedProject) throw new SetupError('project_required');
    const projectId = this.#projectId(id);
    await this.project(id);
    const short = id.slice(0, 12), bucketName = `momso-backup-${short}`;
    const storage = `https://storage.googleapis.com/storage/v1/b/${bucketName}`;
    const key = `https://cloudkms.googleapis.com/v1/projects/${projectId}/locations/${REGION}/keyRings/momso-backup-${short}/cryptoKeys/backup`;
    let bucket = null, slots = null, versions = null, project = null;
    try {
      const info = await this.#request(storage, { allowMissing: true });
      if (info) {
        if (info.name !== bucketName || info.labels?.['momso-install-id'] !== id) throw new SetupError('resource_ownership');
        bucket = { ownership: 'owned', location: info.location };
        const collect = async query => {
          let page = '', count = 0, objects = [], seen = new Set();
          do {
            if (seen.has(page) || ++count > 1000) throw new SetupError('google_response');
            seen.add(page);
            const params = new URLSearchParams(query);
            if (page) params.set('pageToken', page);
            const response = await this.#request(`${storage}/o?${params}`);
            if (response.items !== undefined && !Array.isArray(response.items)) throw new SetupError('google_response');
            objects.push(...(response.items ?? []).map(item => ({ name: item.name, updated: item.updated, size: item.size, storageClass: item.storageClass })));
            if (objects.length > 100000) throw new SetupError('google_response');
            page = response.nextPageToken ?? '';
          } while (page);
          return objects;
        };
        const live = await collect({ versions: 'true' });
        slots = { complete: true, items: live, softDeletedComplete: false };
        try { const soft = await collect({ softDeleted: 'true' }); slots.softDeletedComplete = true; slots.softDeletedCount = soft.length; slots.softDeletedItems = soft; } catch { /* unknown is not empty */ }
      }
    } catch (error) { if (error?.step === 'resource_ownership') throw error; }
    try {
      const info = await this.#request(key, { allowMissing: true });
      if (info) {
        if (info.labels?.['momso-install-id'] !== id || !info.name?.endsWith(`/keyRings/momso-backup-${short}/cryptoKeys/backup`)) throw new SetupError('resource_ownership');
        const response = await this.#request(`${key}/cryptoKeyVersions`);
        if (response.nextPageToken || !Array.isArray(response.cryptoKeyVersions)) throw new SetupError('google_response');
        versions = { complete: true, items: response.cryptoKeyVersions.map(item => ({ state: item.state, protectionLevel: item.protectionLevel })) };
      }
    } catch (error) { if (error?.step === 'resource_ownership') throw error; }
    try {
      const value = await this.#request(`${CRM}projects/${projectId}`);
      project = { createdByMomso: value.projectId === `momso-${id}` && value.labels?.['momso-install-id'] === id, exclusive: null };
    } catch { /* ownership remains unknown */ }
    const output = deletionPreview({ bucket, slots, versions, project });
    output.projectExclusivity = await this.projectExclusivity(id);
    output.projectClosure = output.projectExclusivity.status === 'not_exclusive' ? 'not_suggested' : 'unverified';
    return output;
  }

  async projectExclusivity(id) {
    if (!this.#selectedProject) throw new SetupError('project_required');
    const projectId = this.#projectId(id);
    const pages = async (base, field, params) => {
      let page = '', items = [], seen = new Set();
      do {
        if (seen.has(page) || seen.size >= 1000) throw new SetupError('google_response');
        seen.add(page);
        const query = new URLSearchParams(params);
        if (page) query.set('pageToken', page);
        const value = await this.#request(`${base}?${query}`);
        if (!value || (value[field] !== undefined && !Array.isArray(value[field]))) throw new SetupError('google_response');
        items.push(...(value[field] ?? []));
        if (items.length > 100000) throw new SetupError('google_response');
        page = value.nextPageToken ?? '';
        if (typeof page !== 'string') throw new SetupError('google_response');
      } while (page);
      return items;
    };
    return inspectProjectExclusivity({ projectId, installationId: id,
      listAssets: () => pages(`${ASSET}projects/${projectId}:searchAllResources`, 'results', { pageSize: '200' }),
      listServices: async () => (await pages(`${USAGE}projects/${projectId}/services`, 'services', { filter: 'state:ENABLED', pageSize: '200' })).map(item => item?.config?.name ?? item?.name?.split('/').at(-1)) });
  }

  async discardBackup(id, previous = null) {
    if (!this.#selectedProject) throw new SetupError('project_required');
    const projectId = this.#projectId(id);
    await this.project(id);
    const vm = `${COMPUTE}projects/${projectId}/zones/${ZONE}/instances/momso-${id.slice(0, 12)}`;
    if (await this.#request(vm, { allowMissing: true })) throw new SetupError('vm_unconfirmed');
    return discardBackup({ projectId, installationId: id, previous, sleep: this.#sleep,
      request: (method, url, body) => this.#request(url, { method, body, allowMissing: method === 'GET' }) });
  }

  async destroy(id, onProgress = () => {}, { onVmState = () => {}, discardData = false } = {}) {
    if (!this.#selectedProject) throw new SetupError('project_required');
    const projectId = this.#projectId(id);
    const short = id.slice(0, 12);
    const name = `momso-${short}`;
    const marker = `momso-install-${id}`;
    const base = `${COMPUTE}projects/${projectId}/`;
    const runBase = `${RUN}projects/${projectId}/locations/${REGION}/services/momso-any-${short}`;
    const items = [
      { key: 'vm', url: `${base}zones/${ZONE}/instances/${name}`, type: 'compute', owns: x => x.labels?.['momso-install-id'] === id && x.name === name && x.disks?.length === 1 && x.disks[0].boot === true && x.disks[0].autoDelete === true && x.disks[0].source?.endsWith('/disks/' + name) && x.serviceAccounts?.length === 1 && x.serviceAccounts[0].email === `momso-vm-${short}@${projectId}.iam.gserviceaccount.com` },
      { key: 'router', url: `${base}regions/${REGION}/routers/${name}-router`, type: 'compute', owns: x => x.name === `${name}-router` && x.description === marker && (x.nats ?? []).every(nat => nat.name === `${name}-nat`) && !(x.bgpPeers?.length) && !(x.interfaces?.length) },
      { key: 'firewall', url: `${base}global/firewalls/${name}-private`, type: 'compute', owns: x => x.name === `${name}-private` && x.description === marker && x.network?.endsWith('/' + name + '-net') },
      { key: 'subnet', url: `${base}regions/${REGION}/subnetworks/${name}-sub`, type: 'compute', owns: x => x.name === `${name}-sub` && x.description === marker && x.network?.endsWith('/' + name + '-net') },
      { key: 'network', url: `${base}global/networks/${name}-net`, type: 'compute', owns: x => x.name === `${name}-net` && x.description === marker },
      // The existing deletion path preserves backups; retaining the vault
      // identity is necessary for the customer's saved credentials to survive.
      ...['transport', 'session'].map(suffix => ({ key: `secret_${suffix}`, url: `${SECRETS}projects/${projectId}/secrets/momso-${suffix}-${short}`, type: 'secret', owns: x => x.labels?.['momso-install-id'] === id && x.name?.endsWith(`/secrets/momso-${suffix}-${short}`) })),
      { key: 'secret_vault', preserve: true, url: `${SECRETS}projects/${projectId}/secrets/momso-vault-${short}`, type: 'secret', owns: x => x.labels?.['momso-install-id'] === id && x.name?.endsWith(`/secrets/momso-vault-${short}`) },
      ...['vm', 'gateway'].map(suffix => ({ key: `account_${suffix}`, url: `${IAM}projects/${projectId}/serviceAccounts/momso-${suffix}-${short}@${projectId}.iam.gserviceaccount.com`, type: 'account', owns: x => x.email === `momso-${suffix}-${short}@${projectId}.iam.gserviceaccount.com` && x.description === marker })),
      ...[['Stop', ['compute.instances.stop']], ['Anywhere', ['compute.instances.get', 'compute.instances.start']], ['Update', ['run.operations.get', 'run.revisions.get', 'run.revisions.list', 'run.services.get', 'run.services.update']]].map(([suffix, permissions]) => ({ key: `role_${suffix.toLowerCase()}`, preserve: !discardData, url: `${IAM}projects/${projectId}/roles/momso${suffix}${short}`, type: 'role', owns: x => x.name?.endsWith(`/roles/momso${suffix}${short}`) && x.description === marker && JSON.stringify([...(x.includedPermissions ?? [])].sort()) === JSON.stringify(permissions) })),
      { key: 'gateway', url: runBase, type: 'run', owns: x => x.labels?.['momso-install-id'] === id && x.name === `projects/${projectId}/locations/${REGION}/services/momso-any-${short}` },
    ];
    await this.project(id);
    for (const item of items) {
      item.before = await this.#request(item.url, { allowMissing: true });
      if (item.type === 'role' && item.before?.deleted) item.before = null;
      if (item.before && !item.owns(item.before)) throw new SetupError('resource_ownership');
    }
    // Remove installation principals before deleting their accounts; all policies are reread.
    const policyResources = [{ read: `${CRM}projects/${projectId}:getIamPolicy`, readMethod: 'POST', readBody: { options: { requestedPolicyVersion: 3 } }, write: `${CRM}projects/${projectId}:setIamPolicy` }];
    const vmItem = items.find(item => item.key === 'vm');
    if (vmItem.before) policyResources.push({ read: `${vmItem.url}/getIamPolicy?optionsRequestedPolicyVersion=3`, write: `${vmItem.url}/setIamPolicy` });
    const gatewayItem = items.find(item => item.key === 'gateway');
    if (gatewayItem.before) policyResources.push({ read: `${runBase}:getIamPolicy?options.requestedPolicyVersion=3`, write: `${runBase}:setIamPolicy` });
    for (const item of items.filter(value => value.before && (value.type === 'secret' || value.type === 'account'))) {
      policyResources.push({ read: `${item.url}:getIamPolicy?options.requestedPolicyVersion=3`, write: `${item.url}:setIamPolicy` });
    }
    const bucket = `https://storage.googleapis.com/storage/v1/b/momso-backup-${short}`;
    let bucketInfo;
    try { bucketInfo = await this.#request(bucket, { allowMissing: true }); }
    catch (error) { if (error?.step === 'authorization_expired') throw error; throw new SetupError('iam_bindings_unverified'); }
    if (bucketInfo) {
      if (bucketInfo.labels?.['momso-install-id'] !== id) throw new SetupError('resource_ownership');
      policyResources.push({ read: `${bucket}/iam?optionsRequestedPolicyVersion=3`, write: `${bucket}/iam`, writeMethod: 'PUT', direct: true });
    }
    const ring = `https://cloudkms.googleapis.com/v1/projects/${projectId}/locations/${REGION}/keyRings/momso-backup-${short}`;
    let ringInfo;
    try { ringInfo = await this.#request(ring, { allowMissing: true }); }
    catch (error) { if (error?.step === 'authorization_expired') throw error; throw new SetupError('iam_bindings_unverified'); }
    if (ringInfo) {
      const key = `${ring}/cryptoKeys/backup`;
      let keyInfo;
      try { keyInfo = await this.#request(key, { allowMissing: true }); }
      catch (error) { if (error?.step === 'authorization_expired') throw error; throw new SetupError('iam_bindings_unverified'); }
      if (!keyInfo || keyInfo.labels?.['momso-install-id'] !== id) throw new SetupError('resource_ownership');
      for (const endpoint of [ring, key]) policyResources.push({ read: `${endpoint}:getIamPolicy?options.requestedPolicyVersion=3`, write: `${endpoint}:setIamPolicy` });
    }
    const emails = ['vm', 'gateway'].map(suffix => `momso-${suffix}-${short}@${projectId}.iam.gserviceaccount.com`);
    try {
      await removeAndVerify({ resources: policyResources, emails,
        read: resource => this.#request(resource.read, { method: resource.readMethod ?? 'GET', body: resource.readBody }),
        write: (resource, policy) => this.#request(resource.write, { method: resource.writeMethod ?? 'POST', body: resource.direct ? policy : { policy } }) });
    } catch (error) { if (error?.step === 'authorization_expired') throw error; throw new SetupError('iam_bindings_unverified'); }
    const remaining = [], retained = [];
    let networkPending = false;
    for (const item of items) {
      if (item.preserve) {
        retained.push({ resource: item.key, status: item.before ? 'retained' : 'unverified', reason: item.before ? 'backup_recovery' : 'not_found_or_unavailable', ...(item.type === 'role' ? { billing: 'free' } : {}) });
        continue;
      }
      if (!item.before) { if (item.key === 'vm') onVmState('deleted'); continue; }
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
        if (item.key === 'vm') onVmState('unknown');
        let op;
        try { op = await this.#request(item.url + etag, { method: 'DELETE' }); }
        catch (error) {
          if (item.key === 'vm' && error?.requestRejected === true) onVmState('failed');
          throw error;
        }
        if (item.type === 'run') await this.#pollRun(projectId, op.name);
        if (item.type === 'compute') {
          try { await this.#computeOperation(projectId, op); }
          catch (error) {
            if (item.key === 'vm' && error?.step === 'operation_failed') {
              const vm = await this.#request(item.url, { allowMissing: true });
              if (vm && item.owns(vm) && vm.status === 'RUNNING' && (!item.before.id || vm.id === item.before.id)) onVmState('failed');
            }
            throw error;
          }
        }
        if (item.key === 'vm' && await this.#request(item.url, { allowMissing: true })) throw new SetupError('vm_deletion_unconfirmed');
        if (item.key === 'vm') onVmState('deleted');
        onProgress({ step: item.key, state: 'done' });
      } catch (error) {
        // Stop only when continuing could be unsafe: the VM may still run,
        // ownership is unproven, or the session ended. Any other Google
        // failure leaves just that resource for the retry, so the gateway,
        // secrets and accounts after it are still deleted (S173).
        const stop = item.key === 'vm' || !(error instanceof SetupError)
          || ['resource_ownership', 'authorization_expired', 'vm_deletion_unconfirmed', 'pause_cancelled', 'project_required'].includes(error.step);
        if (stop) throw error;
        remaining.push(item.key); onProgress({ step: item.key, state: 'pending' });
        // The network can be deleted only after its router, firewall and subnet.
        if (['router', 'firewall', 'subnet', 'network'].includes(item.key)) networkPending = true;
      }
    }
    retained.push({ resource: 'backup_bucket', status: 'unverified', reason: 'not_inspected' },
      { resource: 'backup_kms', status: 'unverified', reason: 'not_inspected' });
    const groups = { vm: ['vm'], gateway: ['gateway'], network: ['router', 'firewall', 'subnet', 'network'],
      secrets: ['secret_transport', 'secret_session'], iam: ['account_vm', 'account_gateway', ...(discardData ? ['role_stop', 'role_anywhere', 'role_update'] : [])] };
    const resources = {};
    for (const [group, keys] of Object.entries(groups)) {
      let found = false, unknown = false;
      for (const key of keys) {
        const item = items.find(value => value.key === key);
        try {
          const current = await this.#request(item.url, { allowMissing: true });
          if (current && !(item.type === 'role' && current.deleted)) {
            if (!item.owns(current)) unknown = true;
            else found = true;
          }
        } catch { unknown = true; }
      }
      const status = found ? 'retained' : unknown ? 'unverified' : 'deleted';
      resources[group] = { status, reason: status === 'deleted' ? null : status === 'retained' ? 'cleanup_pending' : 'inventory_unavailable' };
      if (found && unknown) resources[group + '_inventory'] = { status: 'unverified', reason: 'inventory_unavailable' };
    }
    try {
      const disk = await this.#request(`${base}zones/${ZONE}/disks/${name}`, { allowMissing: true });
      resources.disk = disk ? { status: 'unverified', reason: 'ownership_unconfirmed' } : { status: 'deleted', reason: null };
    } catch { resources.disk = { status: 'unverified', reason: 'inventory_unavailable' }; }
    resources.iam_bindings = { status: 'deleted', reason: null };
    return { remaining, mode: 'pause', status: 'backup_freshness_unverified', retained, resources, billingZero: null };
  }
}
