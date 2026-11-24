import test from 'node:test';
import assert from 'node:assert/strict';
import { inspectProjectExclusivity } from './momso_anywhere_project_exclusivity.mjs';
import { GoogleBrowserProvider } from './momso_anywhere_google_browser.mjs';

const projectId = 'fixture-project', id = 'a'.repeat(24);
const owned = { assetType: 'compute.googleapis.com/Instance', name: `//compute.googleapis.com/projects/${projectId}/zones/x/instances/momso-${id.slice(0, 12)}`, labels: { 'momso-install-id': id } };

test('owned or empty inventories cannot prove exclusivity', async () => {
  for (const assets of [[], [owned]]) {
    const value = await inspectProjectExclusivity({ projectId, installationId: id, listAssets: async () => assets, listServices: async () => ['compute.googleapis.com'] });
    assert.equal(value.status, 'unverified');
    assert.deepEqual(value.checks, { assets: 'observed', services: 'observed' });
    assert(value.limitations.includes('unsupported_asset_types'));
  }
});

test('another VM, bucket or labelled installation prevents a closure suggestion', async () => {
  for (const asset of [
    { assetType: 'compute.googleapis.com/Instance', name: '//compute.googleapis.com/projects/x/zones/z/instances/foreign-vm' },
    { assetType: 'storage.googleapis.com/Bucket', name: '//storage.googleapis.com/another-bucket' },
    { assetType: 'run.googleapis.com/Service', name: '//run.googleapis.com/projects/x/services/other', labels: { 'momso-install-id': 'b'.repeat(24) } },
  ]) {
    const value = await inspectProjectExclusivity({ projectId, installationId: id, listAssets: async () => [owned, asset], listServices: async () => { throw new Error('denied'); } });
    assert.equal(value.status, 'not_exclusive');
    assert.equal(value.checks.services, 'unverified');
    assert(value.observedTypes.includes(asset.assetType));
  }
});

test('unavailable, malformed and unrecognized services never prove ownership', async () => {
  const input = { projectId, installationId: id, listAssets: async () => { throw new Error('denied'); }, listServices: async () => ['example.googleapis.com'] };
  const value = await inspectProjectExclusivity(input);
  assert.equal(value.status, 'unverified');
  assert.deepEqual(value.enabledServices, ['example.googleapis.com']);
  assert.deepEqual(value.unattributedServices, ['example.googleapis.com']);
  assert.equal(value.checks.assets, 'unverified');
  const bad = await inspectProjectExclusivity({ ...input, listAssets: async () => [{ name: 'bad' }], listServices: async () => ['wrong'] });
  assert.equal(bad.checks.assets, 'unverified');
  assert.equal(bad.checks.services, 'unverified');
  const synchronous = await inspectProjectExclusivity({ ...input, listAssets: () => { throw new Error('denied'); } });
  assert.equal(synchronous.status, 'unverified');
});

test('browser preview inventories with GET only, preserving partial errors', async () => {
  const calls = [], fetchImpl = async (raw, options) => {
    const url = new URL(raw); calls.push([url, options.method]);
    assert.equal(options.headers.Authorization, 'Bearer fixture-token');
    if (url.hostname === 'cloudasset.googleapis.com') return Response.json({ results: [owned, { assetType: 'compute.googleapis.com/Instance', name: '//compute.googleapis.com/projects/x/zones/z/instances/foreign-vm' }] });
    if (url.hostname === 'serviceusage.googleapis.com') return Response.json({ services: [{ config: { name: 'compute.googleapis.com' } }] });
    throw new Error('unexpected');
  };
  const provider = new GoogleBrowserProvider('fixture-token', { projectId, fetchImpl });
  const value = await provider.projectExclusivity(id);
  assert.equal(value.status, 'not_exclusive');
  assert.deepEqual(value.enabledServices, ['compute.googleapis.com']);
  assert(calls.every(([, method]) => method === 'GET'));
  assert(calls.every(([url]) => !url.pathname.includes(':delete')));
});

test('unselected project does not send a request', async () => {
  let called = false;
  const provider = new GoogleBrowserProvider('fixture-token', { fetchImpl: async () => { called = true; } });
  await assert.rejects(provider.projectExclusivity(id), { step: 'project_required' });
  assert.equal(called, false);
});

test('browser reads every inventory page and notes non-installer services', async () => {
  const fetchImpl = async raw => {
    const url = new URL(raw);
    if (url.hostname === 'cloudasset.googleapis.com') return Response.json(url.searchParams.has('pageToken') ? { results: [owned] } : { results: [], nextPageToken: 'next' });
    if (url.hostname === 'serviceusage.googleapis.com') return Response.json({ services: [{ config: { name: 'compute.googleapis.com' } }, { config: { name: 'bigquery.googleapis.com' } }] });
    throw new Error('unexpected');
  };
  const value = await new GoogleBrowserProvider('fixture-token', { projectId, fetchImpl }).projectExclusivity(id);
  assert.deepEqual(value.observedTypes, ['compute.googleapis.com/Instance']);
  assert.deepEqual(value.unattributedServices, ['bigquery.googleapis.com']);
  assert.equal(value.status, 'unverified');
});
