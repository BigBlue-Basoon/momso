const aliases = { gateway_unhealthy: 'gateway_probe_failed', gateway_health_failed: 'gateway_probe_failed',
  gateway_traffic_unknown: 'gateway_traffic_mismatch', gateway_rollback_mismatch: 'gateway_rollback_failed',
  gateway_restore_failed: 'gateway_rollback_failed', gateway_address_changed: 'gateway_address_invalid', gateway_address: 'gateway_address_invalid' };
const retries = { gateway_image_permission: false, gateway_image_invalid: false, gateway_image_mismatch: false,
  gateway_traffic_mismatch: true, gateway_probe_failed: true, gateway_rollback_failed: false, gateway_not_ready: true,
  gateway_operation_failed: true, gateway_response_invalid: true, gateway_address_invalid: false, gateway_ownership: false, gateway_policy: false };

export function gatewayFailure(code, rolledBack = null) {
  code = aliases[code] ?? code;
  const recognized = Object.hasOwn(retries, code);
  return { code: recognized ? code : 'gateway_operation_failed', rolledBack, retryable: rolledBack === false ? false : rolledBack === null ? null : recognized ? retries[code] : null };
}
