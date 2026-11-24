// The installer supplies an access token on stdin, never in arguments or output.
import { pathToFileURL } from 'node:url';
import { discardBackup } from './momso_anywhere_discard_backup.mjs';
export { discardBackup } from './momso_anywhere_discard_backup.mjs';

async function main() {
  let data = '';
  for await (const part of process.stdin) { data += part; if (data.length > 8192) throw new Error('invalid'); }
  const { token, projectId, installationId, previous } = JSON.parse(data);
  if (!/^[a-z][a-z0-9-]{4,28}[a-z0-9]$/.test(projectId ?? '') || !/^[a-f0-9]{12}(?:[a-f0-9]{12})?$/.test(installationId ?? '') || typeof token !== 'string' || !token || /\s/.test(token)) throw new Error('invalid');
  const request = async (method, raw, body) => {
    const url = new URL(raw);
    if (url.protocol !== 'https:' || !['storage.googleapis.com', 'cloudkms.googleapis.com', 'secretmanager.googleapis.com'].includes(url.hostname) || url.username || url.password || url.hash) throw new Error('invalid');
    const response = await fetch(url, { method, headers: { Authorization: `Bearer ${token}`, ...(body ? { 'Content-Type': 'application/json' } : {}) }, ...(body ? { body: JSON.stringify(body) } : {}), redirect: 'error', signal: AbortSignal.timeout(30000) });
    if (response.status === 404 && method === 'GET') return null;
    if (!response.ok) throw new Error('unavailable');
    return response.status === 204 ? {} : response.json();
  };
  console.log(JSON.stringify(await discardBackup({ projectId, installationId, request, previous })));
}
if (process.argv[1] && import.meta.url === pathToFileURL(process.argv[1]).href) main().catch(() => { process.stderr.write('cleanup_unavailable\n'); process.exitCode = 1; });
