// Browser-path rehearsal. Tokens stay in memory; never log raw errors.
import { randomBytes, webcrypto, createPublicKey } from 'node:crypto';
import { spawnSync } from 'node:child_process';
import { readFileSync } from 'node:fs';
import { dirname, join, resolve } from 'node:path';
import { pathToFileURL } from 'node:url';
import { startFromGesture, projectsFromGesture, resumeFromRedirect, destroyFromGesture, clearStartAuthorization } from './momso_anywhere_start.mjs';
import { providerFromToken } from './momso_anywhere_google_browser.mjs';
import { verifyRelease } from './momso_release_manifest.mjs';
import { googleBrowserInventory, REQUIRED_RESOURCES } from './momso_anywhere_rehearsal_s117.mjs';
import { cleanupTestBackup } from './momso_anywhere_test_backup_cleanup_s117.mjs';
import { temporaryIapDiagnostics } from './momso_anywhere_rehearsal_diagnostics_s117c.mjs';
import { deletionResult } from './momso_anywhere_deletion_contract.mjs';

const STEPS = ['project', 'billing', 'services', 'privateNetwork', 'server', 'gateway', 'gatewayReady'];
const SAFE_STEPS = new Set([...STEPS, 'invalid_arguments', 'invalid_account', 'invalid_scope', 'authorization_unavailable', 'gcloud_unavailable', 'authorization_expired', 'permission_denied', 'identity_unavailable', 'release_not_ready', 'network', 'server_not_ready', 'gateway_not_ready', 'gateway_image_mismatch', 'gateway_probe_failed', 'resource_ownership', 'project_unavailable', 'billing_not_ready', 'operation_failed', 'operation_timeout', 'interrupted_for_rehearsal']);

export function validateScope(projectId, id, protectedInstallationIds = []) {
  if (!/^[a-z][a-z0-9-]{4,28}[a-z0-9]$/.test(projectId ?? '') || !/^[a-f0-9]{24}$/.test(id ?? '') || !Array.isArray(protectedInstallationIds) || protectedInstallationIds.some(value => !/^[a-f0-9]{12}([a-f0-9]{12})?$/.test(value) || value.slice(0, 12) === id.slice(0, 12))) throw new Error('invalid_scope');
}

export function storageAdapter() {
  const values = new Map();
  return { getItem: key => values.get(key) ?? null, setItem: (key, value) => values.set(key, String(value)), removeItem: key => values.delete(key) };
}

// Wrap the actual provider methods; the second call is the unmodified path.
export function interruptGatewayOnce(provider) {
  let interrupted = false;
  const wrapped = { preflight: provider.preflight.bind(provider), gatewayUrl: provider.gatewayUrl.bind(provider) };
  for (const step of STEPS) wrapped[step] = async id => {
    if (step === 'gateway' && !interrupted) { interrupted = true; throw new Error('interrupted_for_rehearsal'); }
    return provider[step](id);
  };
  return wrapped;
}

export async function runBrowserTrial({ projectId, installationId, protectedInstallationIds = [], start, verify, destroy, inventory, backupCleanup = async () => 'unknown', onProgress = () => {} }) {
  validateScope(projectId, installationId, protectedInstallationIds);
  if ([start, verify, destroy, inventory, backupCleanup, onProgress].some(fn => typeof fn !== 'function')) throw new Error('invalid_arguments');
  const ctx = Object.freeze({ projectId, installationId });
  const result = { installationId, interruption: 'unverified', resume: 'unverified', checks: {}, failure: null, cleanup: 'unverified', backupCleanup: 'unknown', resources: {} };
  try {
    try { await start(ctx, true, onProgress); throw new Error('interruption_missing'); }
    catch (error) { if (error?.step !== 'gateway' && error?.message !== 'interrupted_for_rehearsal') throw error; result.interruption = 'passed'; }
    await start(ctx, false, onProgress);
    result.resume = 'passed';
    result.checks = await verify(ctx);
    if (['traffic_digest', 'public_https', 'vm_services', 'update_status'].some(key => result.checks?.[key] !== 'passed')) result.failure = 'verification_incomplete';
  } catch (error) {
    result.failure = SAFE_STEPS.has(error?.step) ? error.step : 'operation_failed';
  } finally {
    let deletion = null;
    try {
      deletion = await destroy(ctx);
      if (deletion?.outcome) {
        result.cleanup = ['deleted', 'deleted_unverified'].includes(deletion.outcome) ? 'passed' : deletion.outcome === 'failed' ? 'failed' : 'incomplete';
        result.unverified = deletion.unverified ?? [];
      } else result.cleanup = Array.isArray(deletion?.remaining) && !deletion.remaining.length && (deletion.mode !== 'delete_all' || deletion.status === 'complete') ? 'passed' : 'incomplete';
    } catch { result.cleanup = 'failed'; }
    if (deletion?.mode === 'delete_all') result.backupCleanup = deletion;
    else if (deletion) try { result.backupCleanup = await backupCleanup(ctx); } catch { result.backupCleanup = 'unknown'; }
    try { result.resources = await inventory(ctx); } catch { result.resources = {}; }
  }
  return result;
}

// A Windows .cmd wrapper needs a shell; call Google's bundled Python directly.
export function accessToken(account, spawn = spawnSync, platform = process.platform) {
  if (!/^[A-Za-z0-9._%+@-]+$/.test(account ?? '')) throw new Error('invalid_account');
  let program = 'gcloud', args = ['auth', 'print-access-token', '--account=' + account, '--quiet'];
  if (platform === 'win32') {
    const where = spawn('where.exe', ['gcloud.cmd'], { encoding: 'utf8', windowsHide: true, timeout: 10000 });
    if (where.status !== 0 || typeof where.stdout !== 'string') throw new Error('gcloud_unavailable');
    const command = where.stdout.split(/\r?\n/).find(Boolean);
    if (!command || !/gcloud\.cmd$/i.test(command)) throw new Error('gcloud_unavailable');
    const root = resolve(dirname(command), '..');
    program = join(root, 'platform', 'bundledpython', 'python.exe');
    args = ['-S', join(root, 'lib', 'gcloud.py'), ...args];
  }
  const env = { ...process.env, CLOUDSDK_CORE_LOG_HTTP: 'false', CLOUDSDK_CORE_VERBOSITY: 'none', CLOUDSDK_CORE_DISABLE_PROMPTS: '1' };
  const result = spawn(program, args, { encoding: 'utf8', windowsHide: true, timeout: 60000, maxBuffer: 65536, env });
  const token = result.stdout?.trim();
  if (result.status !== 0 || !token || /\s/.test(token)) throw new Error('authorization_unavailable');
  return token;
}

function makeLocation() {
  return { origin: 'https://rehearsal.invalid', href: 'https://rehearsal.invalid/start/', assign() {} };
}

async function bootstrapGesture({ clientId, release, token, storage, session, location, fetchImpl, providerFactory }) {
  clearStartAuthorization();
  const opts = { clientId, release, storage, session, location, cryptoImpl: webcrypto, fetchImpl, providerFactory };
  const redirect = await projectsFromGesture(opts);
  if (!redirect?.redirecting) throw new Error('authorization_unavailable');
  const pending = JSON.parse(session.getItem('momso.start.oauth.pending.v1') ?? 'null');
  if (!pending?.state) throw new Error('authorization_unavailable');
  // This is a local redirect substitute, not evidence of actual browser scopes.
  const fragment = new URLSearchParams({ state: pending.state, access_token: token, token_type: 'Bearer', expires_in: '3000', scope: 'openid email https://www.googleapis.com/auth/cloud-platform' });
  const response = await resumeFromRedirect({ ...opts, fragment: '#' + fragment.toString() });
  if (response.action !== 'projects' || !Array.isArray(response.value)) throw new Error('project_unavailable');
  return response.value;
}

function safeFetch(token, fetchImpl) {
  return async (url, options = {}) => {
    const headers = { ...(options.headers ?? {}), Authorization: `Bearer ${token}` };
    return fetchImpl(url, { ...options, headers, redirect: 'error', cache: 'no-store' });
  };
}

export function guardedFetch(fetchImpl, projectId, protectedInstallationIds = []) {
  return async (input, options = {}) => {
    const url = new URL(input);
    const method = (options.method ?? 'GET').toUpperCase();
    const pathname = decodeURIComponent(url.pathname);
    if (method !== 'GET' && protectedInstallationIds.some(id => pathname.toLowerCase().includes(id.slice(0, 12)))) throw new Error('protected_installation');
    if (method === 'DELETE' && url.hostname === 'cloudresourcemanager.googleapis.com' &&
        /^\/v[13]\/projects\/[^/]+\/?$/.test(pathname)) throw new Error('protected_project');
    return fetchImpl(input, { ...options, signal: options.signal ?? AbortSignal.timeout(30000) });
  };
}

export async function discoverProtectedInstallations(projectId, token, fetchImpl = fetch) {
  if (!/^[a-z][a-z0-9-]{4,28}[a-z0-9]$/.test(projectId ?? '') || !token) throw new Error('invalid_scope');
  const result = new Set();
  const endpoints = [
    `https://compute.googleapis.com/compute/v1/projects/${projectId}/aggregated/instances`,
    `https://run.googleapis.com/apis/serving.knative.dev/v1/namespaces/${projectId}/services`,
  ];
  for (const [index, endpoint] of endpoints.entries()) {
    let page = '';
    do {
      const url = new URL(endpoint);
      if (page) url.searchParams.set(index === 0 ? 'pageToken' : 'continue', page);
      const response = await fetchImpl(url.href, { headers: { Authorization: `Bearer ${token}` }, redirect: 'error', cache: 'no-store', signal: AbortSignal.timeout(30000) });
      if (!response.ok) throw new Error('protected_inventory_unavailable');
      const data = await response.json();
      let rows;
      if (index === 0) {
        if (!data.items || typeof data.items !== 'object' || Array.isArray(data.items)) throw new Error('protected_inventory_unavailable');
        rows = Object.values(data.items).flatMap(group => {
          if (!group || (group.instances !== undefined && !Array.isArray(group.instances))) throw new Error('protected_inventory_unavailable');
          return group.instances ?? [];
        });
      } else {
        if ((data.items !== undefined && !Array.isArray(data.items)) || (data.unreachable !== undefined && (!Array.isArray(data.unreachable) || data.unreachable.length))) throw new Error('protected_inventory_unavailable');
        rows = data.items ?? [];
      }
      for (const row of rows) {
        const name = index === 0 ? row?.name : row?.metadata?.name;
        if (typeof name !== 'string') throw new Error('protected_inventory_unavailable');
        const match = name.split('/').at(-1).match(/^momso-(?:any-)?([a-f0-9]{12})$/);
        if (match) result.add(match[1]);
      }
      page = index === 0 ? (data.nextPageToken ?? '') : (data.metadata?.continue ?? '');
      if (typeof page !== 'string') throw new Error('protected_inventory_unavailable');
    } while (page);
  }
  return [...result];
}

export async function verifyBrowserInstallation({ projectId, installationId, protectedInstallationIds = [] }, { token, signed, account, fetchImpl = fetch, diagnostics = temporaryIapDiagnostics }) {
  validateScope(projectId, installationId, protectedInstallationIds);
  const result = { traffic_digest: 'unverified', public_https: 'unverified', vm_services: 'unverified', update_status: 'unverified' };
  const name = `momso-any-${installationId.slice(0, 12)}`;
  const endpoint = `https://run.googleapis.com/v2/projects/${projectId}/locations/asia-northeast3/services/${name}`;
  const api = safeFetch(token, fetchImpl);
  try {
    const response = await api(endpoint);
    if (!response.ok) throw new Error('gateway_unavailable');
    const service = await response.json();
    if (service.labels?.['momso-install-id'] !== installationId) throw new Error('ownership_unconfirmed');
    const traffic = (service.trafficStatuses ?? []).filter(item => item.percent > 0);
    if (traffic.length === 1 && traffic[0].percent === 100 && /^[a-z0-9-]+$/.test(traffic[0].revision ?? '')) {
      const revisionResponse = await api(`${endpoint}/revisions/${traffic[0].revision}`);
      if (revisionResponse.ok) {
        const revision = await revisionResponse.json();
        if (!revision.reconciling && revision.conditions?.some(item => item.type === 'Ready' && item.state === 'CONDITION_SUCCEEDED') && revision.containers?.length === 1 && revision.containers[0].image === signed.gatewayImage) result.traffic_digest = 'passed';
      }
    }
    const uri = new URL(service.uri);
    if (uri.protocol === 'https:' && uri.hostname.endsWith('.run.app') && uri.origin === service.uri && !uri.username && !uri.password) {
      const publicResponse = await fetchImpl(uri.origin + '/__momso/ready', { cache: 'no-store', credentials: 'omit', redirect: 'error', signal: AbortSignal.timeout(20000) });
      if (publicResponse.status === 200) result.public_https = 'passed';
    }
  } catch { /* inaccessible or malformed data stays unverified */ }
  // The mobile install has no SSH ingress: explicitly create and remove a
  // narrowly scoped diagnostic rule only after the original path has run.
  if (account) {
    const diag = await diagnostics({ account, projectId, installationId, protectedInstallationIds }, { token, fetchImpl });
    result.vm_services = diag.vm_services === 'passed' ? 'passed' : 'unverified';
    result.update_status = diag.update_status === 'passed' ? 'passed' : 'unverified';
    result.diagnosticFirewall = diag.diagnosticFirewall;
    if (diag.diagnosticRule) result.diagnosticRule = diag.diagnosticRule;
    if (diag.diagnosticFirewall !== 'removed') {
      result.vm_services = 'unverified';
      result.update_status = 'unverified';
    }
  }
  return result;
}

export async function cleanupBackup({ projectId, installationId, protectedInstallationIds = [] }, token, fetchImpl, sleep) {
  validateScope(projectId, installationId, protectedInstallationIds);
  const short = installationId.slice(0, 12);
  const bucket = `https://storage.googleapis.com/storage/v1/b/momso-backup-${short}`;
  const api = safeFetch(token, fetchImpl);
  const probe = await api(bucket);
  if (probe.status === 404) return 'unknown'; // A 404 can conceal permissions.
  if (!probe.ok) return 'unknown';
  const request = async (method, url, body) => {
    const target = new URL(url);
    if (!['storage.googleapis.com', 'cloudkms.googleapis.com'].includes(target.hostname) || target.protocol !== 'https:' || target.username || target.password || target.hash) throw new Error('invalid_scope');
    const response = await api(url, { method, ...(body ? { headers: { 'Content-Type': 'application/json' }, body: JSON.stringify(body) } : {}) });
    if (!response.ok) throw new Error('cleanup_unavailable');
    return response.status === 204 ? {} : response.json();
  };
  const value = await cleanupTestBackup({ projectId, installationId, protectedIds: protectedInstallationIds, verifyFresh: async ctx => ctx.projectId === projectId && ctx.installationId === installationId, request, sleep });
  // The ordinary pause path deliberately retains the vault identity. Only this
  // freshly generated disposable trial may remove it after backup deletion.
  value.vault = 'unverified';
  try {
    const secret = `https://secretmanager.googleapis.com/v1/projects/${projectId}/secrets/momso-vault-${short}`;
    const response = await api(secret);
    if (response.ok) {
      const metadata = await response.json();
      if (metadata.name === `projects/${projectId}/secrets/momso-vault-${short}` && metadata.labels?.['momso-install-id'] === installationId && typeof metadata.etag === 'string' && metadata.etag) {
        const removed = await api(`${secret}?etag=${encodeURIComponent(metadata.etag)}`, { method: 'DELETE' });
        if (removed.ok) value.vault = 'delete_requested';
      }
    }
  } catch { /* missing permission is not proof of deletion */ }
  try {
    const path = `https://cloudkms.googleapis.com/v1/projects/${projectId}/locations/asia-northeast3/keyRings/momso-backup-${short}/cryptoKeys/backup/cryptoKeyVersions`;
    const versions = await request('GET', path);
    if (versions.nextPageToken || !Array.isArray(versions.cryptoKeyVersions)) throw new Error('versions_unavailable');
    value.keyVersions = versions.cryptoKeyVersions.map(version => {
      const state = version.state;
      const billing = state === 'DESTROYED' ? 'not_active' : ['ENABLED', 'DISABLED', 'DESTROY_SCHEDULED'].includes(state) ? 'billable' : 'unknown';
      const until = typeof version.destroyTime === 'string' && /^\d{4}-\d\d-\d\dT/.test(version.destroyTime) ? version.destroyTime : null;
      return { state: ['ENABLED', 'DISABLED', 'DESTROY_SCHEDULED', 'DESTROYED'].includes(state) ? state : 'unknown', billing, until };
    });
  } catch { value.keyVersions = [{ state: 'unknown', billing: 'unknown', until: null }]; }
  value.deletion = deletionResult({ mode: 'delete_all', inventoryComplete: false,
    resources: { bucket: { status: 'unverified', reason: 'soft_delete_not_verified' },
      kms: { status: 'retained', reason: 'keyring_pending' },
      secrets: { status: value.vault === 'delete_requested' ? 'unverified' : 'retained', reason: 'vault_delete_not_verified' } },
    billable: value.keyVersions.filter(item => item.billing === 'billable').map(item => ({ resource: 'kms', endsAt: item.until })) });
  return value;
}

export async function executeBrowserRehearsal({ account, projectId, installationId, protectedInstallationIds = [], clientId, release, token, fetchImpl = fetch, providerFactory = providerFromToken, onProgress = () => {} }) {
  validateScope(projectId, installationId, protectedInstallationIds);
  if (!/^[A-Za-z0-9._-]+\.apps\.googleusercontent\.com$/.test(clientId ?? '') || !token || /\s/.test(token)) throw new Error('invalid_arguments');
  if (!protectedInstallationIds.length) protectedInstallationIds = await discoverProtectedInstallations(projectId, token, fetchImpl);
  validateScope(projectId, installationId, protectedInstallationIds);
  fetchImpl = guardedFetch(fetchImpl, projectId, protectedInstallationIds);
  const storage = storageAdapter(), session = storageAdapter(), location = makeLocation();
  const options = { clientId, release, storage, session, location, cryptoImpl: webcrypto, fetchImpl, providerFactory };
  try {
    const projects = await bootstrapGesture({ ...options, token });
    if (!projects.some(item => item?.projectId === projectId)) throw new Error('project_unavailable');
    const identity = await providerFactory({ token, expiresAt: Date.now() + 3000000, clientId, release, projectId, fetchImpl, trustStorage: storage });
    if (!/^[0-9]{1,40}$/.test(identity.ownerSub ?? '')) throw new Error('identity_unavailable');
    const hash = await webcrypto.subtle.digest('SHA-256', new TextEncoder().encode(`${clientId}:${identity.ownerSub}:${projectId}`));
    storage.setItem(`momso-setup-${Buffer.from(hash).toString('hex')}`, installationId);
    const interrupted = interruptGatewayOnce(identity.provider);
    const signed = await verifyRelease(release, { fetchImpl, cryptoImpl: webcrypto, storage });
    const progress = event => { if (STEPS.includes(event?.step) && ['working', 'done', 'failed'].includes(event?.state)) onProgress({ step: event.step, state: event.state }); };
    const start = async (_ctx, inject) => startFromGesture({ ...options, projectId, onProgress: progress, authorize: async () => ({ ownerSub: identity.ownerSub, provider: inject ? interrupted : identity.provider }) });
    const destroy = async ctx => { validateScope(ctx.projectId, ctx.installationId, protectedInstallationIds); return destroyFromGesture({ ...options, projectId: ctx.projectId, installationId: ctx.installationId, discardData: true }); };
    return await runBrowserTrial({ projectId, installationId, protectedInstallationIds, start, destroy,
      verify: ctx => verifyBrowserInstallation({ ...ctx, protectedInstallationIds }, { account, token, signed, fetchImpl }),
      inventory: ctx => googleBrowserInventory(ctx, { token, fetchImpl }),
      onProgress: progress });
  } finally { clearStartAuthorization(); }
}

function parseArgs(argv) {
  const allowed = new Set(['--account', '--project', '--manifest-url', '--public-key', '--channel', '--google-client-id', '--login-origin', '--release-root']);
  const result = { protectedInstallationIds: [] };
  for (let i = 0; i < argv.length; i += 2) {
    if (argv[i] === '--protect-installation-id') {
      if (!/^[a-f0-9]{12}([a-f0-9]{12})?$/.test(argv[i + 1] ?? '')) throw new Error('invalid_arguments');
      result.protectedInstallationIds.push(argv[i + 1]);
      continue;
    }
    if (!allowed.has(argv[i]) || !argv[i + 1] || result[argv[i]]) throw new Error('invalid_arguments');
    result[argv[i]] = argv[i + 1];
  }
  if ([...allowed].some(name => name !== '--release-root' && !result[name]) || !['production', 'development'].includes(result['--channel']) || (result['--channel'] === 'production' && !result['--release-root'])) throw new Error('invalid_arguments');
  return result;
}

async function main() {
  try {
    const args = parseArgs(process.argv.slice(2));
    const installationId = randomBytes(12).toString('hex');
    validateScope(args['--project'], installationId);
    const origin = new URL(args['--login-origin']);
    if (origin.protocol !== 'https:' || origin.origin !== args['--login-origin'] || origin.username || origin.password) throw new Error('invalid_arguments');
    const key = createPublicKey(readFileSync(args['--public-key'])).export({ type: 'spki', format: 'der' }).toString('base64');
    const release = { manifestUrl: args['--manifest-url'], publicKeySpki: key, channel: args['--channel'], loginOrigin: origin.origin };
    if (args['--channel'] === 'production') release.trustBootstrap = readFileSync(args['--release-root']).toString('base64');
    const token = accessToken(args['--account']);
    console.log('시험 설치 ID:', installationId);
    const result = await executeBrowserRehearsal({ account: args['--account'], projectId: args['--project'], installationId, protectedInstallationIds: args.protectedInstallationIds, clientId: args['--google-client-id'], release, token,
      onProgress: value => console.log(JSON.stringify(value)) });
    console.log('설치 ID:', result.installationId);
    console.log('중단·이어하기:', result.interruption, result.resume, '확인:', JSON.stringify(result.checks), '삭제:', result.cleanup);
    if (result.unverified?.length) console.log('삭제 후 미확인:', result.unverified.join(', '));
    for (const kind of REQUIRED_RESOURCES) console.log(kind + ':', ({ present: '남음', absent: '없음', unknown: '미확인' })[result.resources?.[kind] ?? 'unknown']);
    console.log('백업:', JSON.stringify(result.backupCleanup));
    const charges = result.backupCleanup?.billable ?? [];
    const unscheduled = Object.entries(result.resources).some(([kind, value]) => value === 'present' &&
      !(['bucket', 'kms'].includes(kind) && charges.some(item => item.resource === kind) && charges.filter(item => item.resource === kind).every(item => typeof item.endsAt === 'string' && Number.isFinite(Date.parse(item.endsAt)))));
    if (result.failure || result.cleanup !== 'passed' || unscheduled) process.exitCode = 1;
  } catch (error) {
    const reason = SAFE_STEPS.has(error?.step) ? error.step : SAFE_STEPS.has(error?.message) ? error.message : 'unverified';
    process.stderr.write(`실패: ${reason}\n`);
    process.exitCode = 1;
  }
}

if (process.argv[1] && import.meta.url === pathToFileURL(resolve(process.argv[1])).href) await main();
