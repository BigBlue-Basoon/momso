import assert from 'node:assert/strict';
import test from 'node:test';
import { REQUIRED_CHECKS, REQUIRED_RESOURCES, PC_REQUIRED_CHECKS, validatePcInstallationId, classifyPcKmsVersion, classifyKmsVersions, pcResourceNames, pcInstallerArgs, verifyPcEvidence, summarizePcInventory, runPcRehearsal, googleBrowserInventory, inventoryBrowserExactNames, runRehearsal } from './momso_anywhere_rehearsal_s117.mjs';

const id = 'a'.repeat(24);
const all = (keys, value) => Object.fromEntries(keys.map(key => [key, value]));

test('PC rehearsal rejects the protected installation before any operation', () => {
  assert.throws(() => validatePcInstallationId('bbbbbbbbbbbb', ['bbbbbbbbbbbb']), /installation_collision/);
  assert.throws(() => validatePcInstallationId('a'.repeat(12), ['a'.repeat(12)]), /installation_collision/);
  assert.throws(() => validatePcInstallationId('a'.repeat(24)), /invalid_installation_id/);
  assert.equal(validatePcInstallationId('a'.repeat(12)), 'a'.repeat(12));
});

test('PC resource names use the installer identifiers, not browser-only names', () => {
  const installation = 'a'.repeat(12), names = pcResourceNames('fixture-project', installation);
  assert.deepEqual(Object.keys(names), REQUIRED_RESOURCES);
  assert.deepEqual(names.firewall, [`momso-${installation}-iap`]);
  assert(names.accounts.includes(`mv-${installation}@fixture-project.iam.gserviceaccount.com`));
  assert(names.accounts.includes(`ma-${installation}@fixture-project.iam.gserviceaccount.com`));
  assert(names.roles.includes(`momsoBackup${installation}`));
  assert.throws(() => pcResourceNames('fixture-project', 'bbbbbbbbbbbb', ['bbbbbbbbbbbb']), /installation_collision/);
});

test('PC installer arguments preserve the account, project and exact protected state boundary', () => {
  const id = 'a'.repeat(12), account = 'fixture@example.invalid', projectId = 'fixture-project';
  const input = { action: 'anywhere', account, projectId, installationId: id, statePath: '/fixture/state.json', state: { version: 1, id, account, project: projectId },
    artifactPath: '/fixture/momso-server', artifactSha256: '1'.repeat(64), webPath: '/fixture/web.tar.gz', webSha256: '2'.repeat(64),
    manifestUrl: 'https://example.invalid/release.json', publicKeyPath: '/fixture/key.pem', channel: 'development',
    googleClientId: 'fixture.apps.googleusercontent.com', ownerSub: '12345', loginOrigin: 'https://login.example.invalid' };
  const args = pcInstallerArgs(input);
  assert.equal(args[0], 'anywhere');
  assert(args.includes('--account') && args.includes(account));
  assert(args.includes('--project') && args.includes(projectId));
  assert(!args.some(value => ['config', 'auth', 'init'].includes(value)));
  assert.deepEqual(pcInstallerArgs({ ...input, action: 'destroy' }), ['destroy', '--account', account, '--project', projectId, '--state', '/fixture/state.json']);
  assert.throws(() => pcInstallerArgs({ ...input, state: { ...input.state, id: 'bbbbbbbbbbbb' } }), /state_mismatch/);
  assert.throws(() => pcInstallerArgs({ ...input, installationId: 'bbbbbbbbbbbb', protectedIds: ['bbbbbbbbbbbb'] }), /installation_collision/);
  assert.throws(() => pcInstallerArgs({ ...input, ownerSub: undefined }), /invalid_configuration/);
});

test('PC verification requires the actual traffic digest, HTTPS, four services and authenticated status', () => {
  const digest = 'a'.repeat(64), image = `ghcr.io/example/gateway@sha256:${digest}`;
  const evidence = { traffic: [{ revision: 'fixture-00001-abc', percent: 100 }], revisionImageDigest: `sha256:${digest}`,
    httpsStatus: 200, httpsVerified: true, updateStatusAuthenticated: true, updateStatusHttp: 200,
    services: { 'momso-server': 'active', 'momso-private-proxy': 'active', 'momso-idle-stop': 'active', 'momso-update': 'active' } };
  assert.deepEqual(verifyPcEvidence(evidence, image), all(PC_REQUIRED_CHECKS, 'passed'));
  assert.equal(verifyPcEvidence({ ...evidence, traffic: [{ ...evidence.traffic[0], percent: 99 }] }, image).traffic_digest, 'unverified');
  assert.equal(verifyPcEvidence({ ...evidence, revisionImageDigest: `sha256:${'b'.repeat(64)}` }, image).traffic_digest, 'unverified');
  assert.equal(verifyPcEvidence({ ...evidence, httpsVerified: false }, image).public_https, 'unverified');
  assert.equal(verifyPcEvidence({ ...evidence, services: { ...evidence.services, 'momso-update': 'failed' } }, image).vm_services, 'unverified');
  assert.equal(verifyPcEvidence({ ...evidence, updateStatusAuthenticated: false }, image).update_status, 'unverified');
  assert.throws(() => verifyPcEvidence(evidence, 'unverified-image'), /invalid_signed_image/);
});

test('PC resource cost remains unknown for incomplete listing or scheduled KMS destruction', () => {
  const installation = 'a'.repeat(12);
  const groups = Object.fromEntries(REQUIRED_RESOURCES.map(kind => [kind, { complete: true, items: [] }]));
  assert.equal(summarizePcInventory(groups, installation).chargeableZero, true);
  groups.kms.items.push({ name: `momso-backup-${installation}/versions/1`, billing: 'may_be_billable', until: '2026-12-23T00:00:00Z' });
  const pending = summarizePcInventory(groups, installation);
  assert.equal(pending.chargeableZero, null);
  assert.equal(pending.chargeable[0].until, '2026-12-23T00:00:00Z');
  groups.kms.complete = false;
  assert.deepEqual(summarizePcInventory(groups, installation).unknown, ['kms']);
  groups.kms = { complete: true, items: [{ name: 'unrelated-secret-value', billing: 'not_billable' }] };
  const sanitized = summarizePcInventory(groups, installation);
  assert.equal(sanitized.chargeableZero, null);
  assert(!JSON.stringify(sanitized).includes('unrelated-secret-value'));
  assert.deepEqual(classifyPcKmsVersion({ state: 'DESTROY_SCHEDULED', destroyTime: '2026-12-23T00:00:00Z' }), { billing: 'billable', until: '2026-12-23T00:00:00Z' });
  assert.equal(classifyPcKmsVersion({ state: 'DESTROYED' }).billing, 'not_billable');
  assert.equal(classifyPcKmsVersion({ state: 'UNRECOGNIZED' }).billing, 'unknown');
});

test('PC rehearsal attempts cleanup and inventory after partial installation failure', async () => {
  const events = [], installationId = 'a'.repeat(12);
  const groups = Object.fromEntries(REQUIRED_RESOURCES.map(kind => [kind, { complete: true, items: [] }]));
  const result = await runPcRehearsal({ projectId: 'fixture-project', installationId,
    preflight: async () => true, record: async ({ stage }) => events.push(stage),
    start: async () => { events.push('start'); throw new Error('fixture-secret'); },
    verify: async () => { throw new Error('unreachable'); },
    destroy: async () => { events.push('destroy'); return { status: 'done' }; },
    inventory: async () => { events.push('inventory'); return groups; },
  });
  assert.deepEqual(events, ['allocated', 'start', 'destroy', 'inventory', 'cleanup_checked']);
  assert.equal(result.failure, 'operation_failed');
  assert.equal(result.complete, false);
  assert(!JSON.stringify(result).includes('fixture-secret'));
  await assert.rejects(runPcRehearsal({ projectId: 'fixture-project', installationId: 'bbbbbbbbbbbb', protectedIds: ['bbbbbbbbbbbb'] }), /installation_collision/);
});
function fixture() {
  const events = [];
  const options = { projectId: 'fixture-project', protectedIds: ['b'.repeat(12)], makeId: () => id,
    start: async () => events.push('start'), verify: async () => all(REQUIRED_CHECKS, 'passed'),
    destroy: async () => { events.push('destroy'); return { status: 'done' }; },
    inventory: async () => all(REQUIRED_RESOURCES, 'absent'), record: async item => events.push(item.stage) };
  return { events, options };
}

test('cleanup and inventory happen after verification failure', async () => {
  const f = fixture();
  f.options.verify = async () => { throw new Error('do not leak this provider text'); };
  const result = await runRehearsal(f.options);
  assert.equal(result.failure, 'operation_failed');
  assert.equal(result.complete, false);
  assert.deepEqual(f.events, ['allocated', 'start', 'started', 'destroy', 'cleanup_checked']);
  assert(!JSON.stringify(result).includes('provider text'));
});

test('cleanup is attempted after a partially created install', async () => {
  const f = fixture(); f.options.start = async () => { throw new Error('fixture'); };
  assert.equal((await runRehearsal(f.options)).complete, false);
  assert(f.events.includes('destroy'));
});

test('inventory still runs when cleanup fails', async () => {
  const f = fixture(); let inspected = false;
  f.options.destroy = async () => { throw new Error('private error'); };
  f.options.inventory = async () => { inspected = true; return all(REQUIRED_RESOURCES, 'unknown'); };
  const result = await runRehearsal(f.options);
  assert(inspected);
  assert.equal(result.complete, false);
  assert.equal(result.cleanup.status, 'failed');
  assert.equal(result.unresolved.length, REQUIRED_RESOURCES.length);
  assert(!JSON.stringify(result).includes('private error'));
});

test('missing checks or inaccessible and retained resources are not declared clean', async () => {
  const f = fixture(); f.options.verify = async () => ({});
  f.options.inventory = async () => ({ ...all(REQUIRED_RESOURCES, 'absent'), bucket: 'present', iam: 'unknown' });
  const result = await runRehearsal(f.options);
  assert.deepEqual(result.unresolved, ['iam', 'bucket']);
  assert.equal(result.failure, 'verification_incomplete');
  assert.equal(result.complete, false);
});

test('protected first twelve characters prevent collision before start', async () => {
  const f = fixture(); f.options.protectedIds = ['a'.repeat(12) + 'b'.repeat(12)];
  await assert.rejects(runRehearsal(f.options), /installation_collision/);
  assert.deepEqual(f.events, []);
});

test('an omitted protected installation list fails before creating anything', async () => {
  const f = fixture(); f.options.protectedIds = [];
  await assert.rejects(runRehearsal(f.options), /invalid_arguments/);
  assert.deepEqual(f.events, []);
});

test('only an entirely confirmed run is complete', async () => {
  assert.equal((await runRehearsal(fixture().options)).complete, true);
});

test('adapter details are never copied into the public result', async () => {
  const f = fixture();
  f.options.verify = async () => ({ ...all(REQUIRED_CHECKS, 'passed'), traffic_digest: 'sensitive-detail' });
  f.options.destroy = async () => ({ status: 'done', detail: 'sensitive-detail' });
  const result = await runRehearsal(f.options);
  assert.equal(result.checks.traffic_digest, 'unverified');
  assert(!JSON.stringify(result).includes('sensitive-detail'));
});

test('exact-name inventory distinguishes unknown permission and retained bucket', async () => {
  const seen = [];
  const result = await inventoryBrowserExactNames({ projectId: 'fixture-project', installationId: id }, async item => {
    seen.push(item);
    if (item.kind === 'bucket') return 'present';
    if (item.kind === 'iam') throw new Error('403 fixture');
    return 'absent';
  });
  assert.equal(result.bucket, 'present');
  assert.equal(result.iam, 'unknown');
  assert.equal(result.vm, 'absent');
  assert(seen.every(item => item.installationId === id && item.projectId === 'fixture-project'));
  assert(seen.find(item => item.kind === 'bucket').name.endsWith(id.slice(0, 12)));
});

test('Google probe keeps denied and hidden resources unknown without exposing tokens', async () => {
  const result = await googleBrowserInventory({ projectId: 'fixture-project', installationId: id }, {
    token: 'fixture-token', fetchImpl: async (url, options) => {
      assert.equal(options.headers.Authorization, 'Bearer fixture-token');
      assert(url.startsWith('https://'));
      if (url.includes('storage.googleapis.com')) return { status: 200, ok: true };
      return { status: url.includes('secretmanager') ? 403 : 404, ok: false };
    },
  });
  assert.equal(result.bucket, 'present');
  assert.equal(result.secrets, 'unknown');
  assert.equal(result.vm, 'unknown');
  assert.equal(result.iam, 'unknown');
  assert(!JSON.stringify(result).includes('fixture-token'));
});

test('scheduled KMS destruction is not falsely classified as free', () => {
  assert.equal(classifyKmsVersions(['DESTROY_SCHEDULED']), 'may_be_billable');
  assert.equal(classifyKmsVersions(['DESTROYED']), 'not_active');
  assert.equal(classifyKmsVersions(['UNRECOGNIZED']), 'unknown');
});
