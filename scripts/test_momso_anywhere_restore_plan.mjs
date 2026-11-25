import assert from 'node:assert/strict';
import test from 'node:test';
import { restorePlan } from './momso_anywhere_restore_plan.mjs';

test('fresh, partial, unknown and complete preserved resources are distinct', () => {
  assert.deepEqual(restorePlan({ vault: 'absent', bucket: 'absent', key: 'absent' }), { restoreAvailable: false, lastBackupAt: null, requiresRestore: false });
  assert.throws(() => restorePlan({ vault: 'absent', bucket: 'ready', key: 'ready' }), { step: 'restore_assets_incomplete' });
  assert.throws(() => restorePlan({ vault: 'ready', bucket: 'unverified', key: 'ready' }), { step: 'restore_assets_unverified' });
  assert.deepEqual(restorePlan({ vault: 'ready', bucket: 'ready', key: 'ready', slots: [] }), { restoreAvailable: false, lastBackupAt: null, requiresRestore: true });
  assert.deepEqual(restorePlan({ vault: 'ready', bucket: 'ready', key: 'ready', slots: [{ name: 'momso-backup/slot-0.enc', updated: '2026-11-01T00:00:00Z' }, { name: 'momso-backup/slot-3.enc', updated: '2026-11-02T00:00:00Z' }] }), { restoreAvailable: true, lastBackupAt: '2026-11-02T00:00:00Z', requiresRestore: true });
});
