import { test } from 'node:test';
import assert from 'node:assert/strict';
import { readFile } from 'node:fs/promises';
import { SetupError, bindCustomRole } from './momso_anywhere_setup.mjs';

test('custom role propagation retries but permission errors stop immediately', async () => {
  let calls = 0; const delays = [];
  assert.equal(await bindCustomRole(async () => {
    if (++calls < 3) throw new SetupError('role_propagating');
    return 'ok';
  }, async delay => delays.push(delay)), 'ok');
  assert.deepEqual(delays, [5000, 5000]);
  calls = 0;
  await assert.rejects(bindCustomRole(async () => { calls++; throw new SetupError('permission_denied'); }, async () => assert.fail()));
  assert.equal(calls, 1);
});

test('browser custom role bindings use the shared helper', async () => {
  const text = await readFile(new URL('./momso_anywhere_google_browser.mjs', import.meta.url), 'utf8');
  const writes = text.split('\n').filter(value => value.includes('setIamPolicy'));
  assert.ok(writes.length >= 4);
  for (const line of writes) {
    // This write grants the built-in serviceAccountUser role, not a custom role.
    if (line.includes('${accountPath}:setIamPolicy')) continue;
    assert.ok(line.includes('bindCustomRole('), line.trim());
  }
  assert.match(text, /accountBinding\.role === 'roles\/iam\.serviceAccountUser'|item\.role === 'roles\/iam\.serviceAccountUser'/);
  const backup = await readFile(new URL('./momso_anywhere_backup_setup.mjs', import.meta.url), 'utf8');
  assert.match(backup, /bindCustomRole\(\(\) => request\(url/);
});
