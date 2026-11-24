import assert from 'node:assert/strict';
import test from 'node:test';
import { deletionPreview, deletionResult } from './momso_anywhere_deletion_contract.mjs';

test('preview shows backup time and partial estimate without inventing storage cost', () => {
  const value = deletionPreview({ bucket: { ownership: 'owned' }, slots: { complete: true, items: [{ name: 'momso-backup/slot-2.enc', updated: '2026-11-24T00:00:00Z' }] }, versions: { complete: true, items: [{ state: 'ENABLED', protectionLevel: 'SOFTWARE' }] } });
  assert.equal(value.backupExists, true);
  assert.equal(value.lastBackupAt, '2026-11-24T00:00:00Z');
  assert.equal(value.estimatedMonthly.kms, 0.06);
  assert.equal(value.estimatedMonthly.total, null);
  assert.equal(value.projectClosure, 'unverified');
});

test('unknown data never becomes no backup or zero cost', () => {
  const value = deletionPreview();
  assert.equal(value.backupExists, null);
  assert.equal(value.estimatedMonthly.kms, null);
  assert.equal(deletionResult({ mode: 'delete_all' }).billingZero, null);
  assert.equal(deletionPreview({ bucket: { ownership: 'owned' }, slots: { complete: true, items: [] } }).backupExists, null);
  assert.equal(deletionPreview({ bucket: { ownership: 'owned' }, slots: { complete: true, items: [], softDeletedComplete: true, softDeletedCount: 0 } }).backupExists, false);
});

test('caller-supplied exclusivity does not become permission to suggest closure', () => {
  assert.equal(deletionPreview({ project: { createdByMomso: true, exclusive: true } }).projectClosure, 'unverified');
});

test('Seoul storage estimate requires every live and soft-deleted generation', () => {
  const input = { bucket: { ownership: 'owned', location: 'ASIA-NORTHEAST3' }, slots: { complete: true, items: [{ name: 'momso-backup/slot-0.enc', size: '1073741824', storageClass: 'STANDARD' }], softDeletedComplete: true, softDeletedItems: [] }, versions: { complete: true, items: [{ state: 'ENABLED', protectionLevel: 'SOFTWARE' }] } };
  const value = deletionPreview(input);
  assert(Math.abs(value.estimatedMonthly.storage - 0.023) < 0.000001);
  assert(Math.abs(value.estimatedMonthly.subtotal - 0.083) < 0.000001);
  assert.equal(value.estimatedMonthly.total, null);
  input.slots.items[0].size = '1';
  assert(deletionPreview(input).estimatedMonthly.storage > 0);
  input.slots.items[0].size = '1073741824';
  input.slots.softDeletedComplete = false;
  assert.equal(deletionPreview(input).estimatedMonthly.storage, null);
  input.slots.softDeletedComplete = true;
  input.slots.softDeletedItems = [{ size: '42' }];
  assert.equal(deletionPreview(input).estimatedMonthly.storage, null);
});

test('scheduled destruction remains billable until the verified time', () => {
  const resources = Object.fromEntries(['vm', 'disk', 'network', 'subnet', 'firewall', 'router', 'nat', 'gateway', 'accounts', 'roles', 'secrets', 'iam', 'bucket', 'kms'].map(name => [name, { status: 'deleted' }]));
  const value = deletionResult({ mode: 'delete_all', resources, inventoryComplete: true, billable: [{ resource: 'kms', endsAt: '2026-12-24T00:00:00Z' }] });
  assert.equal(value.billingZero, false);
  assert.equal(value.estimatedZeroAt, '2026-12-24T00:00:00Z');
  assert.equal(deletionResult({ mode: 'delete_all', inventoryComplete: false, billable: [{ resource: 'kms' }] }).estimatedZeroAt, null);
  delete resources.bucket;
  assert.equal(deletionResult({ mode: 'delete_all', resources, inventoryComplete: true }).billingZero, null);
  resources.bucket = { status: 'retained' };
  assert.equal(deletionResult({ mode: 'pause', resources, inventoryComplete: true }).billingZero, null);
});
