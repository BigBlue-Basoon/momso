import assert from 'node:assert/strict';
import { spawnSync } from 'node:child_process';
import test from 'node:test';
import { gatewayFailure } from './momso_anywhere_gateway_failure.mjs';

test('Python and browser classify gateway failures identically', () => {
  const codes = ['gateway_image_permission', 'gateway_image_mismatch', 'gateway_traffic_mismatch', 'gateway_traffic_unknown', 'gateway_probe_failed', 'gateway_unhealthy', 'gateway_rollback_failed', 'gateway_rollback_mismatch', 'gateway_not_ready', 'gateway_image_invalid', 'gateway_operation_failed', 'gateway_response_invalid', 'gateway_address_changed', 'gateway_ownership', 'gateway_policy', 'unknown'];
  const values = codes.flatMap(code => [null, true, false].map(rollback => [code, rollback]));
  const source = "import json,sys;sys.path.insert(0,'apps/alpha/desktop/src-tauri/momso-server-update');from cloud_run_gateway import gateway_failure;print(json.dumps([gateway_failure(*v) for v in json.load(sys.stdin)]))";
  const run = spawnSync(process.platform === 'win32' ? 'python' : 'python3', ['-c', source], { input: JSON.stringify(values), encoding: 'utf8', env: { ...process.env, PYTHONDONTWRITEBYTECODE: '1' } });
  assert.equal(run.status, 0, run.stderr);
  assert.deepEqual(JSON.parse(run.stdout), values.map(([code, rollback]) => gatewayFailure(code, rollback)));
});
