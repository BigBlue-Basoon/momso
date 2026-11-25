import assert from 'node:assert/strict';
import test from 'node:test';
import { GoogleBrowserProvider } from './momso_anywhere_google_browser.mjs';

const id = 'a'.repeat(24), short = id.slice(0, 12), project = 'fixture-project';
const response = (data, status = 200) => ({ ok: status >= 200 && status < 300, status, json: async () => data });

function fixture({ vault = true, bucket = true, key = true, otherVm = false } = {}) {
  const calls = [];
  const provider = new GoogleBrowserProvider('fixture', { projectId: project, fetchImpl: async (raw, options) => {
    const url = new URL(raw), path = url.pathname;
    calls.push(options.method);
    assert.equal(options.method, 'GET');
    if (url.hostname === 'compute.googleapis.com') return path.includes('/aggregated/instances') ? response({ items: otherVm ? { 'zones/another': { instances: [{ labels: { 'momso-install-id': id } }] } } : {} }) : response({}, 404);
    if (url.hostname === 'secretmanager.googleapis.com') {
      if (!vault) return response({}, 404);
      return path.endsWith('/versions') ? response({ versions: [{ name: 'projects/fixture/secrets/vault/versions/1', state: 'ENABLED' }] }) : response({ labels: { 'momso-install-id': id } });
    }
    if (url.hostname === 'cloudkms.googleapis.com') {
      if (!key) return response({}, 404);
      return path.endsWith('/cryptoKeyVersions') ? response({ cryptoKeyVersions: [{ state: 'ENABLED' }] }) : response({ labels: { 'momso-install-id': id } });
    }
    if (url.hostname === 'storage.googleapis.com') {
      if (!bucket) return response({}, 404);
      return path.endsWith('/o') ? response({ items: [{ name: 'momso-backup/slot-0.enc', updated: '2026-11-20T01:02:03Z' }] }) : response({ name: `momso-backup-${short}`, labels: { 'momso-install-id': id } });
    }
    assert.fail(raw);
  } });
  return { provider, calls };
}

test('preserved resources are inspected without creating anything', async () => {
  const { provider, calls } = fixture();
  assert.deepEqual(await provider.restorePlan(id), { restoreAvailable: true, lastBackupAt: '2026-11-20T01:02:03Z', requiresRestore: true });
  assert(calls.length >= 6 && calls.every(method => method === 'GET'));
});

test('partial preserved resources fail before any create', async () => {
  const { provider, calls } = fixture({ vault: false });
  await assert.rejects(provider.restorePlan(id), { step: 'restore_assets_incomplete' });
  assert(calls.every(method => method === 'GET'));
});

test('a VM of the same installation in another zone prevents restoring', async () => {
  const { provider, calls } = fixture({ otherVm: true });
  await assert.rejects(provider.restorePlan(id), { step: 'restore_vm_exists' });
  assert(calls.every(method => method === 'GET'));
});

test('discovers two paused installations by their full ownership labels', async () => {
  const ids = ['a'.repeat(24), 'b'.repeat(12)], calls = [];
  const provider = new GoogleBrowserProvider('fixture', { projectId: project, fetchImpl: async (raw, options) => {
    calls.push(options.method);
    const url = new URL(raw), path = url.pathname;
    const labels = id => ({ 'momso-install-id': id });
    if (url.hostname === 'cloudresourcemanager.googleapis.com') return response({ name: 'projects/123456789' });
    if (url.hostname === 'storage.googleapis.com') {
      const found = ids.find(id => path.includes(`momso-backup-${id.slice(0, 12)}`));
      if (!found) return response({ items: ids.map(id => ({ name: `momso-backup-${id.slice(0, 12)}`, labels: labels(id) })) });
      return path.endsWith('/o') ? response({ items: [{ name: 'momso-backup/slot-0.enc', updated: '2026-11-20T01:02:03Z' }] }) : response({ name: `momso-backup-${found.slice(0, 12)}`, labels: labels(found) });
    }
    if (url.hostname === 'secretmanager.googleapis.com') {
      const found = ids.find(id => path.includes(`momso-vault-${id.slice(0, 12)}`));
      if (!found) return response({ secrets: ids.map(id => ({ name: `projects/${project}/secrets/momso-vault-${id.slice(0, 12)}`, labels: labels(id) })) });
      return path.endsWith('/versions') ? response({ versions: [{ name: 'projects/fixture/secrets/vault/versions/1', state: 'ENABLED' }] }) : response({ labels: labels(found) });
    }
    if (url.hostname === 'compute.googleapis.com') return path.includes('/aggregated/instances') ? response({ items: {} }) : response({}, 404);
    if (path.endsWith('/keyRings')) return response({ keyRings: ids.map(id => ({ name: `projects/${project}/locations/asia-northeast3/keyRings/momso-backup-${id.slice(0, 12)}` })) });
    const found = ids.find(id => path.includes(`momso-backup-${id.slice(0, 12)}`));
    if (path.endsWith('/cryptoKeyVersions')) return response({ cryptoKeyVersions: [{ state: 'ENABLED' }] });
    const key = { name: `projects/${project}/locations/asia-northeast3/keyRings/momso-backup-${found.slice(0, 12)}/cryptoKeys/backup`, labels: labels(found) };
    return path.endsWith('/cryptoKeys') ? response({ cryptoKeys: [key] }) : response(key);
  } });
  assert.deepEqual(await provider.discoverPaused(), [
    { installationId: ids[0], assets: 'complete', browserIdCompatible: true, restoreAvailable: true, lastBackupAt: '2026-11-20T01:02:03Z', requiresRestore: true },
    { installationId: ids[1], assets: 'complete', browserIdCompatible: true, restoreAvailable: true, lastBackupAt: '2026-11-20T01:02:03Z', requiresRestore: true },
  ]);
  assert(calls.every(method => method === 'GET'));
});
