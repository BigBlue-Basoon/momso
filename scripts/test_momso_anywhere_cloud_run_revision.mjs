import assert from 'node:assert/strict';
import test from 'node:test';
import { activatePinnedRevision } from './momso_anywhere_cloud_run_revision.mjs';

const image = 'registry.example.test/momso/gateway@sha256:' + 'a'.repeat(64);
const traffic = [{ revision: 'old-rev', percent: 100 }];

function fixture({ mismatch = false, failSwitch = false, ignoreSwitch = false, mismatchAfterSwitch = false, probeFailure = false, staleCreated = false, cached = false, platformDigest = false } = {}) {
  let service = { etag: 'v1', invokerIamDisabled: true, terminalCondition: { state: 'CONDITION_SUCCEEDED' }, latestReadyRevision: 'old-rev', latestCreatedRevision: 'old-rev', trafficStatuses: traffic };
  const writes = [];
  return {
    get service() { return service; }, writes,
    async run() {
      return activatePinnedRevision({ image, service, template: { containers: [{ image }] }, read: async () => service,
        revision: async () => ({ conditions: [{ type: 'Ready', state: 'CONDITION_SUCCEEDED', revisionReason: 'RETIRED' }], containers: [{ image: mismatch || (mismatchAfterSwitch && service.trafficStatuses[0].revision === 'new-rev') ? 'wrong' : platformDigest ? 'cache.region-docker.pkg.dev/ghcr.io/example/image@sha256:' + 'b'.repeat(64) : cached ? 'cache.region-docker.pkg.dev/ghcr.io/example/image@' + image.split('@')[1] : image }] }),
        probe: async () => !probeFailure,
        patch: async (body, mask) => {
          writes.push({ body, mask });
          if (failSwitch && mask === 'traffic,invokerIamDisabled' && body.traffic[0].revision === 'new-rev') throw new Error('fixture failure');
          if (ignoreSwitch && mask === 'traffic,invokerIamDisabled' && body.traffic[0].revision === 'new-rev') return;
          service = { ...service, ...body, etag: 'v2', ...(body.template && !staleCreated ? { latestCreatedRevision: 'new-rev' } : {}), ...(body.traffic ? { trafficStatuses: body.traffic } : {}) };
        } });
    },
  };
}

test('stages pinned image without moving traffic, then promotes it', async () => {
  const f = fixture();
  const result = await f.run();
  assert.equal(f.writes[0].body.traffic[0].revision, 'old-rev');
  assert.equal(f.writes[0].body.template.containers[0].image, image);
  assert.equal(result.trafficStatuses[0].revision, 'new-rev');
});

test('accepts Cloud Run cache location for the same digest', async () => {
  assert.equal((await fixture({ cached: true }).run()).trafficStatuses[0].revision, 'new-rev');
});

test('rejects index digest resolved to a different platform manifest', async () => {
  const f = fixture({ platformDigest: true });
  await assert.rejects(f.run(), { code: 'gateway_image_mismatch', rolledBack: true });
  assert.equal(f.service.trafficStatuses[0].revision, 'old-rev');
});

for (const option of [{ mismatch: true }, { failSwitch: true }, { ignoreSwitch: true }, { mismatchAfterSwitch: true }, { probeFailure: true }, { staleCreated: true }]) test(`failure restores prior traffic ${JSON.stringify(option)}`, async () => {
  const f = fixture(option);
  await assert.rejects(f.run(), error => {
    assert.equal(error.rolledBack, true);
    if (option.mismatch) assert.equal(error.code, 'gateway_image_mismatch');
    if (option.ignoreSwitch) assert.equal(error.code, 'gateway_traffic_mismatch');
    if (option.probeFailure) assert.equal(error.code, 'gateway_probe_failed');
    return true;
  });
  assert.equal(f.service.trafficStatuses[0].revision, 'old-rev');
  assert.equal(f.service.invokerIamDisabled, true);
});

test('invalid image is rejected before changing traffic', async () => {
  let writes = 0;
  await assert.rejects(activatePinnedRevision({ image: 'registry.example.test/momso:latest', service: { trafficStatuses: traffic }, patch: async () => writes++ }), { step: 'gateway_image_invalid' });
  assert.equal(writes, 0);
});

test('reports when previous traffic could not be restored', async () => {
  let service = { etag: 'v1', invokerIamDisabled: true, terminalCondition: { state: 'CONDITION_SUCCEEDED' }, latestReadyRevision: 'old-rev', latestCreatedRevision: 'old-rev', trafficStatuses: traffic };
  await assert.rejects(activatePinnedRevision({ image, service, template: { containers: [{ image }] }, read: async () => service,
    revision: async () => ({ conditions: [{ type: 'Ready', state: 'CONDITION_SUCCEEDED' }], containers: [{ image: 'wrong' }] }), probe: async () => true,
    patch: async body => { service = { ...service, ...body, latestReadyRevision: 'new-rev', latestCreatedRevision: 'new-rev', trafficStatuses: [{ revision: 'foreign-rev', percent: 100 }] }; },
  }), { step: 'gateway_rollback_failed', code: 'gateway_rollback_failed', rolledBack: false, retryable: false });
});
