// Keep all non-installation principals and conditional metadata untouched.
export function withoutAccounts(policy, emails) {
  if (!policy || typeof policy !== 'object' || typeof policy.etag !== 'string' || !policy.etag || !Array.isArray(policy.bindings ?? [])) throw new Error('iam_bindings_unverified');
  if (!Object.hasOwn(policy, 'bindings')) return { ...policy };
  const names = new Set(emails.map(email => `serviceAccount:${email}`));
  const isTarget = value => {
    if (typeof value !== 'string') throw new Error('iam_bindings_unverified');
    const live = value.startsWith('deleted:') ? value.slice(8).replace(/\?uid=[0-9]+$/, '') : value;
    return names.has(live);
  };
  const bindings = (policy.bindings ?? []).map(binding => {
    if (!binding || !Array.isArray(binding.members)) throw new Error('iam_bindings_unverified');
    return { ...binding, members: binding.members.filter(member => !isTarget(member)) };
  }).filter(binding => binding.members.length);
  return { ...policy, bindings };
}

export async function removeAndVerify({ resources, emails, read, write }) {
  for (const resource of resources) {
    const before = await read(resource);
    if (!before) throw new Error('iam_bindings_unverified');
    const after = withoutAccounts(before, emails);
    if (JSON.stringify(after) !== JSON.stringify(before)) await write(resource, after);
    const verified = await read(resource);
    if (!verified || JSON.stringify(withoutAccounts(verified, emails)) !== JSON.stringify(verified)) throw new Error('iam_bindings_unverified');
  }
}
