// Orchestration contract for a disposable install; cloud adapters must enforce ownership.
import { randomBytes } from 'node:crypto';

const REQUIRED_CHECKS = ['traffic_digest', 'public_https', 'vm_services', 'update_status', 'idle_stop', 'wake'];
const REQUIRED_RESOURCES = ['vm', 'disk', 'network', 'subnet', 'firewall', 'router', 'nat', 'gateway', 'accounts', 'roles', 'secrets', 'iam', 'bucket', 'kms'];

// The PC installer uses 12 characters, unlike the browser installer's 24.
export const PC_REQUIRED_CHECKS = ['traffic_digest', 'public_https', 'vm_services', 'update_status'];

export function validatePcInstallationId(id, protectedIds = []) {
  if (!/^[a-f0-9]{12}$/.test(id ?? '') || !Array.isArray(protectedIds) ||
      protectedIds.some(value => !/^[a-f0-9]{12}$/.test(value))) throw new Error('invalid_installation_id');
  if (protectedIds.includes(id)) throw new Error('installation_collision');
  return id;
}

export function classifyPcKmsVersion(version) {
  if (!version || typeof version !== 'object') return { billing: 'unknown', until: null };
  if (version.state === 'DESTROYED') return { billing: 'not_billable', until: null };
  if (['ENABLED', 'DISABLED', 'DESTROY_SCHEDULED'].includes(version.state)) {
    const until = version.state === 'DESTROY_SCHEDULED' && typeof version.destroyTime === 'string' && /^\d{4}-\d\d-\d\dT/.test(version.destroyTime) ? version.destroyTime : null;
    return { billing: 'billable', until };
  }
  return { billing: 'unknown', until: null };
}

// Exact names from GoogleProvider, GoogleAnywhere, VM services and backup
// provisioner. This list is only a query plan, not proof of ownership.
export function pcResourceNames(projectId, installationId, protectedIds = []) {
  if (!/^[a-z][a-z0-9-]{4,28}[a-z0-9]$/.test(projectId ?? '')) throw new Error('invalid_project');
  validatePcInstallationId(installationId, protectedIds);
  const base = `momso-${installationId}`;
  const accounts = ['mv', 'ma', 'mb'].map(prefix => `${prefix}-${installationId}@${projectId}.iam.gserviceaccount.com`);
  const roles = ['momsoStop', 'momsoAnywhere', 'momsoBackup', 'momsoUpdate'].map(prefix => `${prefix}${installationId}`);
  return {
    vm: [base], disk: [base], network: [`${base}-net`], subnet: [`${base}-sub`],
    firewall: [`${base}-iap`], router: [`${base}-router`], nat: [`${base}-nat`],
    gateway: [`momso-any-${installationId}`], accounts, roles,
    secrets: ['transport', 'session', 'vault'].map(type => `momso-${type}-${installationId}`),
    iam: accounts, bucket: [`momso-backup-${installationId}`], kms: [`momso-backup-${installationId}`],
  };
}

// Build argument arrays only; do not invoke a shell or run the installer here.
// The Google login fields are required by the current PC installer and must be
// provided by a trusted local configuration, not guessed from a live account.
export function pcInstallerArgs({ action, account, projectId, installationId, protectedIds = [], statePath, state, artifactPath, artifactSha256, webPath, webSha256, manifestUrl, publicKeyPath, channel, googleClientId, ownerSub, loginOrigin }) {
  validatePcInstallationId(installationId, protectedIds);
  if (!/^[A-Za-z0-9._%+@-]+$/.test(account ?? '') || !/^[a-z][a-z0-9-]{4,28}[a-z0-9]$/.test(projectId ?? '') ||
      typeof statePath !== 'string' || !statePath || statePath.includes('\0') || !['anywhere', 'destroy'].includes(action)) throw new Error('invalid_arguments');
  if (state?.version !== 1 || state.id !== installationId || state.account !== account || state.project !== projectId) throw new Error('state_mismatch');
  const args = [action, '--account', account, '--project', projectId, '--state', statePath];
  if (action === 'destroy') return args;
  let manifest, origin;
  try { manifest = new URL(manifestUrl); origin = new URL(loginOrigin); } catch { throw new Error('invalid_configuration'); }
  if (!['production', 'development'].includes(channel) || !/^[a-f0-9]{64}$/.test(artifactSha256 ?? '') || !/^[a-f0-9]{64}$/.test(webSha256 ?? '') ||
      !/^[A-Za-z0-9._-]+\.apps\.googleusercontent\.com$/.test(googleClientId ?? '') || !/^[0-9]{1,40}$/.test(ownerSub ?? '') ||
      manifest.protocol !== 'https:' || !manifest.hostname || manifest.username || manifest.password || manifest.hash ||
      origin.protocol !== 'https:' || !origin.hostname || origin.username || origin.password || origin.pathname !== '/' || origin.search || origin.hash ||
      [artifactPath, webPath, publicKeyPath].some(value => typeof value !== 'string' || !value || value.includes('\0'))) throw new Error('invalid_configuration');
  return [...args, '--artifact', artifactPath, '--sha256', artifactSha256, '--web-archive', webPath, '--web-sha256', webSha256,
    '--google-client-id', googleClientId, '--owner-sub', ownerSub, '--login-origin', origin.origin,
    '--release-manifest-url', manifest.href, '--release-public-key', publicKeyPath, '--release-channel', channel];
}

// Pure validation: the caller is responsible for obtaining each value from
// the actual traffic revision, HTTPS response and the private VM respectively.
export function verifyPcEvidence(evidence, expectedImage) {
  if (typeof expectedImage !== 'string' || !/^[a-zA-Z0-9._/-]+@sha256:[a-f0-9]{64}$/.test(expectedImage)) throw new Error('invalid_signed_image');
  const expected = expectedImage.split('@')[1];
  const traffic = evidence?.traffic;
  const validTraffic = Array.isArray(traffic) && traffic.length === 1 &&
    traffic[0]?.percent === 100 && typeof traffic[0]?.revision === 'string' &&
    /^[a-z][a-z0-9-]{0,62}$/.test(traffic[0].revision) &&
    typeof evidence?.revisionImageDigest === 'string' &&
    evidence.revisionImageDigest.split('@').at(-1) === expected;
  const services = ['momso-server', 'momso-private-proxy', 'momso-idle-stop', 'momso-update'];
  return {
    traffic_digest: validTraffic ? 'passed' : 'unverified',
    public_https: evidence?.httpsStatus === 200 && evidence?.httpsVerified === true ? 'passed' : 'unverified',
    vm_services: services.every(name => evidence?.services?.[name] === 'active') ? 'passed' : 'unverified',
    update_status: evidence?.updateStatusAuthenticated === true && evidence?.updateStatusHttp === 200 ? 'passed' : 'unverified',
  };
}

// Only separately verified listings can establish absence. A failed or partial
// listing always leaves the cost answer unknown rather than claiming zero.
export function summarizePcInventory(groups, installationId) {
  validatePcInstallationId(installationId);
  const retained = [], chargeable = [], unknown = [];
  for (const kind of REQUIRED_RESOURCES) {
    const group = groups?.[kind];
    if (group?.complete !== true || !Array.isArray(group.items)) { unknown.push(kind); continue; }
    for (const item of group.items) {
      if (typeof item?.name !== 'string' || !/^[a-zA-Z0-9._/@:-]{1,512}$/.test(item.name) || !item.name.toLowerCase().includes(installationId) ||
          !['billable', 'may_be_billable', 'not_billable', 'unknown'].includes(item.billing)) {
        unknown.push(kind); continue;
      }
      const value = { kind, name: item.name };
      if (item.billing === 'unknown') { unknown.push(kind); retained.push(value); continue; }
      retained.push(value);
      if (item.billing !== 'not_billable') {
        chargeable.push({ ...value, billing: item.billing, until: typeof item.until === 'string' && /^\d{4}-\d\d-\d\dT/.test(item.until) ? item.until : null });
      }
    }
  }
  return { retained, chargeable, unknown: [...new Set(unknown)], chargeableZero: unknown.length || chargeable.some(item => item.billing === 'may_be_billable') ? null : chargeable.length === 0 };
}

// PC-only orchestration skeleton. None of these callbacks is an authorization
// to delete; a live adapter must independently prove ownership before changes.
export async function runPcRehearsal({ projectId, installationId, protectedIds = [], preflight, start, verify, destroy, inventory, record = async () => {} }) {
  if (!/^[a-z][a-z0-9-]{4,28}[a-z0-9]$/.test(projectId ?? '')) throw new Error('invalid_project');
  validatePcInstallationId(installationId, protectedIds);
  if ([preflight, start, verify, destroy, inventory, record].some(fn => typeof fn !== 'function')) throw new Error('adapter_incomplete');
  const context = Object.freeze({ projectId, installationId });
  if (await preflight(context) !== true) throw new Error('ownership_unconfirmed');
  await record({ ...context, stage: 'allocated' });
  let checks = {}, failure = null, cleanup = 'not_started', groups = {};
  try {
    await start(context);
    await record({ ...context, stage: 'started' });
    const observed = await verify(context);
    checks = Object.fromEntries(PC_REQUIRED_CHECKS.map(key => [key, observed?.[key] === 'passed' ? 'passed' : 'unverified']));
    if (PC_REQUIRED_CHECKS.some(key => checks[key] !== 'passed')) failure = 'verification_incomplete';
  } catch { failure = 'operation_failed'; }
  finally {
    try { cleanup = (await destroy(context))?.status === 'done' ? 'done' : 'incomplete'; }
    catch { cleanup = 'failed'; }
    try { groups = await inventory(context); } catch { groups = {}; }
    try { await record({ ...context, stage: 'cleanup_checked', failure, cleanup }); }
    catch { failure ??= 'record_unavailable'; }
  }
  const remaining = summarizePcInventory(groups, installationId);
  return { ...context, checks, failure, cleanup, remaining,
    complete: failure === null && cleanup === 'done' && remaining.unknown.length === 0 && remaining.retained.length === 0 };
}

export function classifyKmsVersions(states) {
  if (!Array.isArray(states) || states.some(state => !['ENABLED', 'DISABLED', 'DESTROY_SCHEDULED', 'DESTROYED'].includes(state))) return 'unknown';
  return states.some(state => state !== 'DESTROYED') ? 'may_be_billable' : 'not_active';
}

// These are browser-path candidate names only; a real adapter must inspect
// ownership, nested IAM/NAT and provider-specific residuals before returning absent.
export async function inventoryBrowserExactNames({ projectId, installationId }, readResource) {
  if (!/^[a-z][a-z0-9-]{4,28}[a-z0-9]$/.test(projectId ?? '') || !/^[a-f0-9]{24}$/.test(installationId ?? '') || typeof readResource !== 'function') throw new Error('invalid_arguments');
  const short = installationId.slice(0, 12), base = `momso-${short}`;
  const names = {
    vm: [base], disk: [base], network: [`${base}-net`], subnet: [`${base}-sub`],
    firewall: [`${base}-private`], router: [`${base}-router`], nat: [`${base}-nat`],
    gateway: [`momso-any-${short}`], accounts: [`momso-vm-${short}`, `momso-gateway-${short}`],
    roles: [`momsoStop${short}`, `momsoAnywhere${short}`, `momsoUpdate${short}`],
    secrets: ['transport', 'session', 'vault'].map(type => `momso-${type}-${short}`),
    iam: [base], bucket: [`momso-backup-${short}`], kms: [`momso-backup-${short}`],
  };
  const states = {};
  for (const [kind, entries] of Object.entries(names)) {
    const results = [];
    for (const name of entries) {
      try {
        const result = await readResource({ projectId, installationId, kind, name });
        results.push(['absent', 'present', 'unknown'].includes(result) ? result : 'unknown');
      } catch { results.push('unknown'); }
    }
    states[kind] = results.includes('present') ? 'present' : results.includes('unknown') ? 'unknown' : 'absent';
  }
  return states;
}

// Read-only browser-path probe. IAM remains unknown until all resource-level
// bindings can also be enumerated; a 403 is never interpreted as absence.
export async function googleBrowserInventory(context, { token, fetchImpl = fetch }) {
  if (typeof token !== 'string' || !token || /\s/.test(token)) throw new Error('invalid_authorization');
  const region = 'asia-northeast3', zone = `${region}-a`;
  return inventoryBrowserExactNames(context, async ({ projectId, kind, name }) => {
    if (kind === 'iam') return 'unknown';
    const compute = `https://compute.googleapis.com/compute/v1/projects/${projectId}/`;
    const urls = {
      vm: `${compute}zones/${zone}/instances/${name}`,
      disk: `${compute}zones/${zone}/disks/${name}`,
      network: `${compute}global/networks/${name}`,
      subnet: `${compute}regions/${region}/subnetworks/${name}`,
      firewall: `${compute}global/firewalls/${name}`,
      router: `${compute}regions/${region}/routers/${name}`,
      nat: `${compute}regions/${region}/routers/momso-${context.installationId.slice(0, 12)}-router`,
      gateway: `https://run.googleapis.com/v2/projects/${projectId}/locations/${region}/services/${name}`,
      accounts: `https://iam.googleapis.com/v1/projects/${projectId}/serviceAccounts/${name}@${projectId}.iam.gserviceaccount.com`,
      roles: `https://iam.googleapis.com/v1/projects/${projectId}/roles/${name}`,
      secrets: `https://secretmanager.googleapis.com/v1/projects/${projectId}/secrets/${name}`,
      bucket: `https://storage.googleapis.com/storage/v1/b/${name}`,
      kms: `https://cloudkms.googleapis.com/v1/projects/${projectId}/locations/${region}/keyRings/${name}`,
    };
    let response;
    try { response = await fetchImpl(urls[kind], { headers: { Authorization: `Bearer ${token}` }, cache: 'no-store', redirect: 'error' }); }
    catch { return 'unknown'; }
    // Some Google APIs hide authorization failures as 404. Absence requires
    // a separately authorized listing, which this read-only probe lacks.
    if (response.status === 404) return 'unknown';
    if (!response.ok) return 'unknown';
    if (kind !== 'nat') return 'present';
    try {
      const body = await response.json();
      if (!Array.isArray(body.nats) && body.nats !== undefined) return 'unknown';
      return (body.nats ?? []).some(nat => nat.name === name) ? 'present' : 'absent';
    } catch { return 'unknown'; }
  });
}

export async function runRehearsal({ projectId, protectedIds, makeId = () => randomBytes(12).toString('hex'), start, verify, destroy, inventory, record = async () => {} }) {
  if (!/^[a-z][a-z0-9-]{4,28}[a-z0-9]$/.test(projectId ?? '') || !Array.isArray(protectedIds) || !protectedIds.length || protectedIds.some(id => !/^[a-f0-9]{12}([a-f0-9]{12})?$/.test(id))) throw new Error('invalid_arguments');
  if ([start, verify, destroy, inventory, record].some(fn => typeof fn !== 'function')) throw new Error('adapter_incomplete');
  const id = makeId();
  if (!/^[a-f0-9]{24}$/.test(id) || protectedIds.some(value => value.slice(0, 12) === id.slice(0, 12))) throw new Error('installation_collision');
  const context = Object.freeze({ projectId, installationId: id });
  let attempted = false, failure = null, checks = {}, cleanup = null, resources = {};
  await record({ ...context, stage: 'allocated' });
  try {
    attempted = true;
    await start(context);
    await record({ ...context, stage: 'started' });
    const observed = await verify(context);
    checks = Object.fromEntries(REQUIRED_CHECKS.map(key => [key, observed?.[key] === 'passed' ? 'passed' : 'unverified']));
    if (REQUIRED_CHECKS.some(key => checks[key] !== 'passed')) failure = 'verification_incomplete';
  } catch {
    failure = 'operation_failed';
  } finally {
    if (attempted) {
      try { cleanup = { status: (await destroy(context))?.status === 'done' ? 'done' : 'incomplete' }; }
      catch { cleanup = { status: 'failed' }; }
      try { resources = await inventory(context); }
      catch { resources = {}; }
      const unresolved = REQUIRED_RESOURCES.filter(key => resources?.[key] !== 'absent');
      await record({ ...context, stage: 'cleanup_checked', failure, unresolved });
    }
  }
  const unresolved = REQUIRED_RESOURCES.filter(key => resources?.[key] !== 'absent');
  return { ...context, checks, failure, cleanup, unresolved, complete: !failure && cleanup?.status === 'done' && !unresolved.length };
}

export { REQUIRED_CHECKS, REQUIRED_RESOURCES };
