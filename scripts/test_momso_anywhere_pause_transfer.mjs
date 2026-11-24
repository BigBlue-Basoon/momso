import assert from 'node:assert/strict';
import test from 'node:test';
import { clearPause, parsePauseFragment, savePauseForRedirect, takePauseAfterRedirect } from './momso_anywhere_pause_transfer.mjs';

const session = () => { const data = new Map(); return { data, getItem: key => data.get(key) ?? null, setItem: (key, value) => data.set(key, value), removeItem: key => data.delete(key) }; };
const value = { ticket: 'a'.repeat(43), gateway: 'https://fixture.run.app', projectId: 'fixture-project', installationId: 'a'.repeat(24), createdAt: 1000, expiresAt: 301000 };
const fragment = () => '#' + new URLSearchParams({ pause_ticket: value.ticket, gateway: value.gateway, project_id: value.projectId,
  installation_id: value.installationId, created_at: String(value.createdAt), expires_at: String(value.expiresAt) });

test('pause fragment is parsed without accepting duplicate or foreign fields', () => {
  assert.deepEqual(parsePauseFragment(fragment(), 2000), value);
  assert.throws(() => parsePauseFragment(fragment() + '&pause_ticket=x', 2000), { step: 'invalid_pause_ticket' });
  assert.throws(() => parsePauseFragment(fragment() + '&other=x', 2000), { step: 'invalid_pause_ticket' });
});

test('redirect checkpoint is consumed once and never restored', () => {
  const store = session();
  savePauseForRedirect(value, store, 2000);
  assert.deepEqual(takePauseAfterRedirect(store, 3000), value);
  assert.equal(takePauseAfterRedirect(store, 3000), null);
  assert.equal(store.data.size, 0);
});

test('expired checkpoint is removed before it is rejected', () => {
  const store = session();
  savePauseForRedirect(value, store, 2000);
  assert.throws(() => takePauseAfterRedirect(store, value.expiresAt), { step: 'invalid_pause_ticket' });
  assert.equal(store.data.size, 0);
});

test('untrusted URL, excessive lifetime and future creation are rejected', () => {
  for (const bad of [{ ...value, gateway: 'https://fixture.run.app.evil.test' }, { ...value, expiresAt: 999999 }, { ...value, createdAt: 3000 }]) {
    assert.throws(() => savePauseForRedirect(bad, session(), 2000), { step: 'invalid_pause_ticket' });
  }
  const store = session(); savePauseForRedirect(value, store, 2000); clearPause(store); assert.equal(store.data.size, 0);
});
