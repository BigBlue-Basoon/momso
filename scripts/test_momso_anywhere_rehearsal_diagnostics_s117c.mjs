import assert from 'node:assert/strict';
import test from 'node:test';
import { existsSync } from 'node:fs';
import { dirname } from 'node:path';
import { sshProbe, temporaryIapDiagnostics } from './momso_anywhere_rehearsal_diagnostics_s117c.mjs';

const context = { account: 'fixture@example.test', projectId: 'fixture-project', installationId: 'a'.repeat(24) };
const short = context.installationId.slice(0, 12), instance = `momso-${short}`;
function fixture({ sshFails = false, foreign = false, lostResponse = false } = {}) {
  let exists = false;
  const calls = [];
  const rule = { id: '123', name: `${instance}-s117c-iap`, description: `momso-s117c-${context.installationId}`, direction: 'INGRESS', network: `projects/${context.projectId}/global/networks/${instance}-net`, sourceRanges: ['35.235.240.0/20'], targetTags: [instance], allowed: [{ IPProtocol: 'tcp', ports: ['22'] }] };
  const reply = (body, status = 200) => new Response(JSON.stringify(body), { status });
  const fetchImpl = async (url, options = {}) => {
    const method = options.method ?? 'GET', path = new URL(url).pathname;
    calls.push([method, path]);
    if (path.endsWith(`/instances/${instance}`)) return reply({ name: instance, labels: { 'momso-install-id': context.installationId }, tags: { items: [instance] }, networkInterfaces: [{ network: rule.network }] });
    if (path.endsWith('/global/firewalls') && method === 'POST') { exists = true; if (lostResponse) throw new Error('connection lost'); return reply({ name: 'op-create' }); }
    if (path.includes('/global/operations/')) return reply({ status: 'DONE' });
    if (path.endsWith(`/global/firewalls/${rule.name}`)) {
      if (method === 'DELETE') { exists = false; return reply({ name: 'op-delete' }); }
      if (!exists) return reply({}, 404);
      return reply(foreign ? { ...rule, description: 'other' } : rule);
    }
    throw new Error('unexpected');
  };
  const ssh = async () => { if (sshFails) throw new Error('private error'); return { vm_services: 'passed', update_status: 'passed' }; };
  return { calls, fetchImpl, ssh, sleep: async () => {}, exists: () => exists };
}

test('diagnostics creates a scoped IAP rule and removes it', async () => {
  const fake = fixture();
  const result = await temporaryIapDiagnostics(context, { token: 'fixture-token', ...fake });
  assert.deepEqual(result, { vm_services: 'passed', update_status: 'passed', diagnosticFirewall: 'removed' });
  assert.equal(fake.exists(), false);
  assert(fake.calls.some(([method, path]) => method === 'DELETE' && path.endsWith(`${instance}-s117c-iap`)));
});

test('diagnostic failure and lost create response still attempt cleanup', async () => {
  for (const option of [{ sshFails: true }, { lostResponse: true }]) {
    const fake = fixture(option);
    const result = await temporaryIapDiagnostics(context, { token: 'fixture-token', ...fake });
    assert.equal(result.vm_services, 'unverified');
    assert.equal(result.diagnosticFirewall, 'removed');
    assert.equal(fake.exists(), false);
  }
});

test('foreign firewall and protected id are never deleted', async () => {
  const fake = fixture({ foreign: true });
  const result = await temporaryIapDiagnostics(context, { token: 'fixture-token', ...fake });
  assert.equal(result.diagnosticFirewall, 'remaining_or_unknown');
  assert(!fake.calls.some(([method]) => method === 'DELETE'));
  const guarded = fixture();
  await assert.rejects(temporaryIapDiagnostics({ ...context, installationId: 'bbbbbbbbbbbb' + 'a'.repeat(12), protectedInstallationIds: ['bbbbbbbbbbbb'] }, { token: 'fixture-token', ...guarded }));
  assert.deepEqual(guarded.calls, []);
});

test('SSH uses a temporary key path and removes it without exposing credentials', () => {
  let key;
  const result = sshProbe(context, { platform: 'linux', spawn: (_program, args) => {
    key = args.find(arg => arg.startsWith('--ssh-key-file=')).split('=').slice(1).join('=');
    assert(!JSON.stringify(args).includes('fixture-token'));
    return { status: 0, stdout: '{"vm_services":"passed","update_status":"passed"}\n' };
  } });
  assert.deepEqual(result, { vm_services: 'passed', update_status: 'passed' });
  assert(key);
  assert.equal(existsSync(key), false);
  assert.equal(existsSync(dirname(key)), false);
});
