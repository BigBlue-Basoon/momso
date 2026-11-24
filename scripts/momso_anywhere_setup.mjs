// The customer-authorized provider owns reconciliation; checkpoints never contain tokens.
import { gatewayFailure } from './momso_anywhere_gateway_failure.mjs';
const STEPS = [
  'project', 'billing', 'services', 'privateNetwork', 'server', 'gateway', 'gatewayReady',
];

export class SetupError extends Error {
  constructor(step) {
    super(`momso anywhere setup failed at ${step}`);
    this.step = step;
  }
}

export async function bindCustomRole(action, sleep = ms => new Promise(resolve => setTimeout(resolve, ms))) {
  for (let attempt = 0; attempt < 12; attempt++) {
    try { return await action(); }
    catch (error) {
      if (!(error instanceof SetupError) || error.step !== 'role_propagating' || attempt === 11) throw error;
      await sleep(5000);
    }
  }
}

export async function startAnywhere(provider, checkpoint, onProgress = () => {}) {
  if (!provider || STEPS.some((step) => typeof provider[step] !== 'function')) {
    throw new SetupError('provider_not_ready');
  }
  if (!checkpoint || !/^[a-f0-9]{24}$/.test(checkpoint.installationId)) {
    throw new SetupError('invalid_checkpoint');
  }
  if (typeof provider.preflight === 'function') await provider.preflight();
  // Even previously completed steps are reconciled: the user may have changed a resource.
  for (const step of STEPS) {
    onProgress({ step, state: 'working' });
    try {
      await provider[step](checkpoint.installationId);
    } catch (error) {
      const details = error instanceof SetupError && typeof error.code === 'string' ? { code: error.code, rolledBack: error.rolledBack ?? null, retryable: error.retryable ?? null } : error instanceof SetupError && error.step.startsWith('gateway_') ? gatewayFailure(error.step) : {};
      onProgress({ step, state: 'failed', ...details });
      const wrapped = new SetupError(details.code ?? (error instanceof SetupError ? error.step : step));
      Object.assign(wrapped, details);
      throw wrapped;
    }
    onProgress({ step, state: 'done' });
  }
  return { url: typeof provider.gatewayUrl === 'function' ? provider.gatewayUrl() : null };
}

// This preflight is deliberately read-only. It is not a complete Google provider.
export async function googleBillingPreflight(project, accessToken, fetchImpl = fetch) {
  if (!/^[a-z][a-z0-9-]{4,28}[a-z0-9]$/.test(project) || !accessToken) {
    throw new SetupError('invalid_authorization');
  }
  let response;
  try {
    response = await fetchImpl(`https://cloudbilling.googleapis.com/v1/projects/${project}/billingInfo`, {
      headers: { Authorization: `Bearer ${accessToken}` },
      cache: 'no-store',
      redirect: 'error',
    });
  } catch {
    throw new SetupError('billing_unreachable');
  }
  if (!response.ok) throw new SetupError('billing_unavailable');
  const result = await response.json();
  return { billingEnabled: result.billingEnabled === true };
}
