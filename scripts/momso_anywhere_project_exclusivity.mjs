// Read-only, deliberately conservative: a search result cannot prove exclusivity.
export const INSTALLER_SERVICES = Object.freeze(['compute.googleapis.com', 'run.googleapis.com', 'secretmanager.googleapis.com', 'iam.googleapis.com', 'serviceusage.googleapis.com', 'storage.googleapis.com', 'cloudkms.googleapis.com']);
export async function inspectProjectExclusivity({ projectId, installationId, listAssets, listServices }) {
  if (!/^[a-z][a-z0-9-]{4,28}[a-z0-9]$/.test(projectId ?? '') || !/^[a-f0-9]{12}(?:[a-f0-9]{12})?$/.test(installationId ?? '') || typeof listAssets !== 'function' || typeof listServices !== 'function') throw new Error('invalid_scope');
  const result = { status: 'unverified', observedTypes: [], enabledServices: [], installerServices: [...INSTALLER_SERVICES].sort(), unattributedServices: [], checks: { assets: 'unverified', services: 'unverified' }, foreignEvidence: [], limitations: ['unsupported_asset_types', 'asset_inventory_delay', 'service_enablement_provenance_unknown', 'iam_and_external_billing_unverified'] };
  const [assets, services] = await Promise.allSettled([Promise.resolve().then(listAssets), Promise.resolve().then(listServices)]);
  if (assets.status === 'fulfilled' && Array.isArray(assets.value)) {
    const valid = assets.value.filter(x => x && typeof x.assetType === 'string' && typeof x.name === 'string' && (x.labels === undefined || (x.labels && typeof x.labels === 'object' && !Array.isArray(x.labels))));
    result.checks.assets = valid.length === assets.value.length ? 'observed' : valid.length ? 'partial' : 'unverified';
    result.observedTypes = [...new Set(valid.map(x => x.assetType))].sort();
    const short = installationId.slice(0, 12);
    for (const item of valid) {
      const marker = item.labels?.['momso-install-id'];
      const otherInstallation = typeof marker === 'string' && /^[a-f0-9]{12}(?:[a-f0-9]{12})?$/.test(marker) && marker !== installationId;
      const last = item.name.split('/').filter(Boolean).at(-1);
      const differentVm = item.assetType === 'compute.googleapis.com/Instance' && last !== `momso-${short}`;
      const differentBucket = item.assetType === 'storage.googleapis.com/Bucket' && last !== `momso-backup-${short}`;
      if (otherInstallation || differentVm || differentBucket) {
        result.foreignEvidence.push({ assetType: item.assetType, reason: otherInstallation ? 'different_installation' : 'other_named_resource' });
      }
    }
  }
  if (services.status === 'fulfilled' && Array.isArray(services.value) && services.value.every(x => typeof x === 'string' && /^[a-z0-9.-]+\.googleapis\.com$/.test(x))) {
    result.checks.services = 'observed';
    result.enabledServices = [...new Set(services.value)].sort();
    result.unattributedServices = result.enabledServices.filter(name => !INSTALLER_SERVICES.includes(name));
  }
  if (result.foreignEvidence.length) result.status = 'not_exclusive';
  return result;
}
