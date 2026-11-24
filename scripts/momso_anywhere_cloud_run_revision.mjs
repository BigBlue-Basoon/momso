// Browser REST counterpart of CloudRunGateway: stage a pinned revision, then switch traffic.
import { SetupError } from './momso_anywhere_setup.mjs';

export function currentTraffic(service) {
  const traffic = (service.trafficStatuses ?? []).filter(item => item.percent > 0).map(item => ({ type: 'TRAFFIC_TARGET_ALLOCATION_TYPE_REVISION', revision: item.revision, percent: item.percent }));
  if (!traffic.length || traffic.some(item => !/^[a-z0-9-]+$/.test(item.revision ?? '') || !Number.isInteger(item.percent)) || traffic.reduce((sum, item) => sum + item.percent, 0) !== 100) throw new SetupError('gateway_traffic_unknown');
  return traffic;
}

export async function activatePinnedRevision({ image, service, template, read, patch, revision, probe }) {
  if (!/^[a-zA-Z0-9._/-]+@sha256:[a-f0-9]{64}$/.test(image ?? '')) throw new SetupError('gateway_image_invalid');
  const previous = currentTraffic(service);
  const previousCreated = service.latestCreatedRevision;
  if (!/^[a-z0-9-]+$/.test(previousCreated ?? '')) throw new SetupError('gateway_not_ready');
  const previousPublic = !!service.invokerIamDisabled;
  let changed = false;
  try {
    changed = true;
    await patch({ etag: service.etag, template, traffic: previous }, 'template,traffic');
    let current = await read();
    const ready = current.latestCreatedRevision;
    if (current.reconciling || current.terminalCondition?.state !== 'CONDITION_SUCCEEDED' || !/^[a-z0-9-]+$/.test(ready ?? '') || ready === previousCreated) throw new SetupError('gateway_not_ready');
    const built = await revision(ready);
    if (built.reconciling || !built.conditions?.some(item => item.type === 'Ready' && item.state === 'CONDITION_SUCCEEDED')) throw new SetupError('gateway_not_ready');
    if (built.containers?.length !== 1 || built.containers[0].image !== image) throw new SetupError('gateway_image_mismatch');
    await patch({ etag: current.etag, traffic: [{ type: 'TRAFFIC_TARGET_ALLOCATION_TYPE_REVISION', revision: ready, percent: 100 }], invokerIamDisabled: true }, 'traffic,invokerIamDisabled');
    current = await read();
    if (current.reconciling || current.terminalCondition?.state !== 'CONDITION_SUCCEEDED' || !current.invokerIamDisabled || currentTraffic(current).some(item => item.revision !== ready)) throw new SetupError('gateway_not_ready');
    const serving = await revision(currentTraffic(current)[0].revision);
    if (serving.containers?.length !== 1 || serving.containers[0].image !== image) throw new SetupError('gateway_image_mismatch');
    if (typeof probe !== 'function' || !await probe()) throw new SetupError('gateway_probe_failed');
    return current;
  } catch (error) {
    if (changed) {
      try {
        const current = await read();
        await patch({ etag: current.etag, traffic: previous, invokerIamDisabled: previousPublic }, 'traffic,invokerIamDisabled');
        const restored = await read();
        const canonical = list => [...list].sort((a, b) => a.revision.localeCompare(b.revision)).map(({ revision, percent }) => ({ revision, percent }));
        if (restored.reconciling || restored.invokerIamDisabled !== previousPublic || JSON.stringify(canonical(currentTraffic(restored))) !== JSON.stringify(canonical(previous))) throw new Error('traffic restore not confirmed');
      } catch { throw new SetupError('gateway_rollback_failed'); }
    }
    throw error;
  }
}
