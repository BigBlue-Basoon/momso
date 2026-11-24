import assert from 'node:assert/strict';
import { spawnSync } from 'node:child_process';
import test from 'node:test';
import { webcrypto } from 'node:crypto';
import { deletionResult } from './momso_anywhere_delete_result.mjs';
import { clearStartAuthorization, destroyFromGesture, resumeFromRedirect } from './momso_anywhere_start.mjs';

const fixtures = [
  { mode: 'delete_all', resources: { bucket: { status: 'deleted', reason: null }, kms: { status: 'deleted', reason: null } }, billable: [], billingZero: null },
  { mode: 'delete_all', resources: { bucket: { status: 'retained', reason: 'soft_delete_retention' }, kms: { status: 'unverified', reason: 'cleanup_unverified' } }, billable: [{ resource: 'bucket', endsAt: '2030-01-01T00:00:00Z' }], billingZero: false },
  { mode: 'pause', retained: [{ resource: 'backup_bucket', status: 'unverified', reason: 'not_inspected' }], remaining: ['network'] },
  { mode: 'pause', retained: [{ resource: 'backup_bucket', status: 'retained', reason: 'backup_recovery' }] },
  { mode: 'pause', failed: true, reason: 'vm_unreachable', resources: {} },
  { mode: 'delete_all', resources: { kms: { status: 'retained', reason: 'destruction_scheduled' } }, billable: [{ resource: 'kms', endsAt: '2030-01-01T00:00:00Z' }, { resource: 'kms', endsAt: null }] },
  { mode: 'delete_all', resources: { kms: { status: 'retained', reason: 'destruction_scheduled' } }, billable: [{ resource: 'kms', endsAt: '2030-02-31T00:00:00Z' }] },
];

test('PC and mobile normalize identical fixtures without treating unknown as deleted', () => {
  for (const fixture of fixtures) {
    const code = "import json,sys; sys.path.insert(0,'scripts'); from momso_anywhere_delete_result import deletion_result; print(json.dumps(deletion_result(json.load(sys.stdin))))";
    const run = spawnSync(process.platform === 'win32' ? 'python' : 'python3', ['-c', code], { input: JSON.stringify(fixture), encoding: 'utf8', env: { ...process.env, PYTHONDONTWRITEBYTECODE: '1' } });
    assert.equal(run.status, 0, run.stderr);
    assert.deepEqual(JSON.parse(run.stdout), deletionResult(fixture));
  }
  assert.equal(deletionResult(fixtures[1]).outcome, 'deleted_unverified');
  assert.deepEqual(deletionResult(fixtures[1]).unverified, ['kms']);
  assert.equal(deletionResult(fixtures[0]).complete, true);
  assert.equal(deletionResult(fixtures[2]).outcome, 'remaining');
  assert.equal(deletionResult(fixtures[4]).outcome, 'failed');
  assert.equal(deletionResult(fixtures[5]).outcome, 'remaining');
  assert.deepEqual(deletionResult(fixtures[5]).remaining, ['kms']);
  assert.equal(deletionResult(fixtures[6]).outcome, 'remaining');
  assert.equal(deletionResult(fixtures[2]).billingZero, null);
});

test('real PC and mobile entry points produce the same result for the same fake resources', async () => {
  const fixture = { mode: 'delete_all', resources: { bucket: { status: 'deleted', reason: null }, kms: { status: 'unverified', reason: 'cleanup_unverified' } }, billable: [], billingZero: null, estimatedZeroAt: null };
  const code = `import io,json,sys,tempfile\nfrom pathlib import Path\nfrom unittest.mock import patch\nsys.path.insert(0,'scripts')\nimport momso_server_install as i, momso_anywhere_install as a, momso_anywhere_pc_discard as d\nfrom test_momso_server_install import FakeProvider\nwith tempfile.TemporaryDirectory() as root:\n p=Path(root)/'state.json'; s=i.initialize_state(p,'fixture@example.test','fixture-project'); s['anywhere']={'url':'https://fixture.example'}; i.save_state(p,s)\n out=io.StringIO()\n with patch.object(i,'GoogleProvider',return_value=FakeProvider()),patch.object(a,'GoogleAnywhere'),patch.object(d,'discard_backup',return_value=json.load(sys.stdin)),patch('sys.stdout',out):\n  code=i.main(['destroy','--account','fixture@example.test','--project','fixture-project','--state',str(p),'--discard-data'])\n assert code==0\n print(out.getvalue(),end='')`;
  const pc = spawnSync(process.platform === 'win32' ? 'python' : 'python3', ['-c', code], { input: JSON.stringify(fixture), encoding: 'utf8', env: { ...process.env, PYTHONDONTWRITEBYTECODE: '1' } });
  assert.equal(pc.status, 0, pc.stderr);
  const mem = () => { const data = new Map(); return { getItem: k => data.get(k) ?? null, setItem: (k,v) => data.set(k,v), removeItem: k => data.delete(k) }; };
  const session = mem(), storage = mem(), clientId = 'fixture.apps.googleusercontent.com';
  const location = { href: 'https://login.example.test/start/', origin: 'https://login.example.test', assign(url) { this.next = url; } };
  const options = { clientId, release: {}, projectId: 'fixture-project', installationId: 'a'.repeat(24), discardData: true, session, storage, location, cryptoImpl: webcrypto,
    providerFactory: async () => ({ ownerSub: '12345', provider: { destroy: async () => ({ remaining: [] }), discardBackup: async () => structuredClone(fixture) } }) };
  clearStartAuthorization(); await destroyFromGesture(options);
  const state = new URL(location.next).searchParams.get('state');
  const fragment = new URLSearchParams({ state, access_token: 'fixture-token', token_type: 'Bearer', expires_in: '60', scope: 'openid email https://www.googleapis.com/auth/cloud-platform' }).toString();
  const mobile = (await resumeFromRedirect({ ...options, fragment })).value;
  assert.deepEqual(mobile, JSON.parse(pc.stdout));
  assert.equal(mobile.outcome, 'deleted_unverified');
  assert(mobile.unverified.includes('kms'));
});
