// Only installation-owned resources are adopted; existing IAM bindings are preserved.
import { SetupError } from './momso_anywhere_setup.mjs';

const STORAGE = 'https://storage.googleapis.com/storage/v1/';
const KMS = 'https://cloudkms.googleapis.com/v1/';
const REGION = 'asia-northeast3';

export async function prepareBackupResources(request, project, installationId) {
  if (!/^[a-z][a-z0-9-]{4,28}[a-z0-9]$/.test(project) || !/^[a-f0-9]{24}$/.test(installationId)) throw new SetupError('invalid_installation');
  const short = installationId.slice(0, 12);
  const bucket = `momso-backup-${short}`;
  const member = `serviceAccount:momso-vm-${short}@${project}.iam.gserviceaccount.com`;
  const bucketUrl = `${STORAGE}b/${bucket}`;
  let storage = await request(bucketUrl, { allowMissing: true });
  if (!storage) {
    await request(`${STORAGE}b?project=${encodeURIComponent(project)}`, { method: 'POST', body: { name: bucket, location: REGION, labels: { 'momso-install-id': installationId }, iamConfiguration: { uniformBucketLevelAccess: { enabled: true }, publicAccessPrevention: 'enforced' } } });
    storage = await request(bucketUrl);
  }
  if (storage.name !== bucket || storage.labels?.['momso-install-id'] !== installationId || storage.iamConfiguration?.uniformBucketLevelAccess?.enabled !== true || storage.iamConfiguration?.publicAccessPrevention !== 'enforced') throw new SetupError('resource_ownership');
  const location = `projects/${project}/locations/${REGION}`;
  const ring = `${location}/keyRings/momso-backup-${short}`;
  const key = `${ring}/cryptoKeys/backup`;
  let keyring = await request(KMS + ring, { allowMissing: true });
  let created = false;
  if (!keyring) {
    await request(`${KMS}${location}/keyRings?keyRingId=momso-backup-${short}`, { method: 'POST', body: {} });
    keyring = await request(KMS + ring);
    created = true;
  }
  if (keyring.name !== ring) throw new SetupError('resource_ownership');
  let cryptoKey = await request(KMS + key, { allowMissing: true });
  if (!cryptoKey) {
    if (!created) throw new SetupError('resource_ownership');
    await request(`${KMS}${ring}/cryptoKeys?cryptoKeyId=backup`, { method: 'POST', body: { purpose: 'ENCRYPT_DECRYPT', versionTemplate: { algorithm: 'GOOGLE_SYMMETRIC_ENCRYPTION' }, labels: { 'momso-install-id': installationId } } });
    cryptoKey = await request(KMS + key);
  }
  if (cryptoKey.name !== key || cryptoKey.purpose !== 'ENCRYPT_DECRYPT' || cryptoKey.labels?.['momso-install-id'] !== installationId) throw new SetupError('resource_ownership');
  async function addBinding(url, role, method, wrap) {
    const policy = await request(url + (wrap ? ':getIamPolicy?options.requestedPolicyVersion=3' : '/iam?optionsRequestedPolicyVersion=3'));
    if (!policy.etag) throw new SetupError('policy_unavailable');
    const bindings = (policy.bindings ?? []).map(value => ({ ...value, members: [...(value.members ?? [])] }));
    let binding = bindings.find(value => value.role === role && !value.condition);
    if (!binding) { binding = { role, members: [] }; bindings.push(binding); }
    binding.members = [...new Set([...binding.members, member])];
    const updated = { ...policy, version: 3, bindings };
    await request(url + (wrap ? ':setIamPolicy' : '/iam'), { method, body: wrap ? { policy: updated } : updated });
  }
  await addBinding(bucketUrl, 'roles/storage.objectAdmin', 'PUT', false);
  await addBinding(KMS + key, 'roles/cloudkms.cryptoKeyEncrypterDecrypter', 'POST', true);
  return { bucket, key };
}
