import assert from 'node:assert/strict';
import test from 'node:test';
import { activatePinnedRevision } from './momso_anywhere_cloud_run_revision.mjs';

const image = 'registry.example.test/momso/gateway@sha256:' + 'a'.repeat(64);
const traffic = [{ revision: 'old-rev', percent: 100 }];

function fixture({ mismatch = false, failSwitch = false } = {}) {
  let service = { etag: 'v1', invokerIamDisabled: true, terminalCondition: { state: 'CONDITION_SUCCEEDED' }, latestReadyRevision: 'old-rev', latestCreatedRevision: 'old-rev', trafficStatuses: traffic };
  const writes = [];
  return {
    get service() { return service; }, writes,
    async run() {
      return activatePinnedRevision({ image, service, template: { containers: [{ image }] }, read: async () => service,
        revision: async () => ({ containers: [{ image: mismatch ? 'wrong' : image }] }),
        patch: async (body, mask) => {
          writes.push({ body, mask });
          if (failSwitch && mask === 'traffic,invokerIamDisabled' && body.traffic[0].revision === 'new-rev') throw new Error('fixture failure');
          service = { ...service, ...body, etag: 'v2', ...(body.template ? { latestReadyRevision: 'new-rev', latestCreatedRevision: 'new-rev' } : {}), ...(body.traffic ? { trafficStatuses: body.traffic } : {}) };
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

for (const option of [{ mismatch: true }, { failSwitch: true }]) test(`failure restores prior traffic ${JSON.stringify(option)}`, async () => {
  const f = fixture(option);
  await assert.rejects(f.run());
  assert.equal(f.service.trafficStatuses[0].revision, 'old-rev');
  assert.equal(f.service.invokerIamDisabled, true);
});

test('invalid image is rejected before changing traffic', async () => {
  let writes = 0;
  await assert.rejects(activatePinnedRevision({ image: 'registry.example.test/momso:latest', service: { trafficStatuses: traffic }, patch: async () => writes++ }), { step: 'gateway_image_invalid' });
  assert.equal(writes, 0);
});
