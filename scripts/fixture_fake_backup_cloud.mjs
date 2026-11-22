export function fakeBackupCloud(project, id, response) {
  const short = id.slice(0, 12), resources = new Map(), policies = new Map();
  const bucket = `/storage/v1/b/momso-backup-${short}`;
  const ring = `/v1/projects/${project}/locations/asia-northeast3/keyRings/momso-backup-${short}`;
  const key = `${ring}/cryptoKeys/backup`;
  return (raw, options) => {
    const url = new URL(raw), path = url.pathname, method = options.method;
    if (!['storage.googleapis.com', 'cloudkms.googleapis.com'].includes(url.hostname)) return undefined;
    const body = options.body ? JSON.parse(options.body) : {};
    if (path.endsWith('/iam') || path.endsWith(':getIamPolicy') || path.endsWith(':setIamPolicy')) {
      const name = path.replace(/\/iam$|:getIamPolicy$|:setIamPolicy$/, '');
      if (method !== 'GET') { policies.set(name, body.policy ?? body); return response({}); }
      return response(policies.get(name) ?? { etag: 'fixture', bindings: [] });
    }
    if (method === 'POST' && path === '/storage/v1/b') { resources.set(bucket, body); return response(body); }
    if (method === 'POST' && path.endsWith('/keyRings')) { const result = { name: ring.slice(4) }; resources.set(ring, result); return response(result); }
    if (method === 'POST' && path.endsWith('/cryptoKeys')) { const result = { ...body, name: key.slice(4) }; resources.set(key, result); return response(result); }
    return resources.has(path) ? response(resources.get(path)) : response({}, 404);
  };
}
