// Temporary IAP-only diagnostics for the new disposable browser installation.
// This does not alter the mobile installer; inability to prove cleanup fails closed.
import { spawnSync } from 'node:child_process';
import { mkdtempSync, rmSync } from 'node:fs';
import { tmpdir } from 'node:os';
import { dirname, join, resolve, sep } from 'node:path';

const region = 'asia-northeast3', zone = region + '-a';
const remote = `import http.client,json,ssl,subprocess,time
names=['momso-server','momso-private-proxy','momso-idle-stop','momso-update']
result={'vm_services':'unverified','update_status':'unverified'}
for attempt in range(30):
 try:
  def active(n):
   p=subprocess.run(['systemctl','is-active',n+'.service'],capture_output=True,timeout=10)
   return p.returncode==0 and p.stdout.strip()==b'active'
  result['vm_services']='passed' if all(active(n) for n in names) else 'unverified'
  token=open('/etc/momso-private/transport-token',encoding='utf-8').read().strip()
  context=ssl.create_default_context(cafile='/etc/momso-private/certificate.pem');context.check_hostname=False
  c=http.client.HTTPSConnection('127.0.0.1',8766,context=context,timeout=10)
  c.request('GET','/update/status',headers={'Authorization':'Bearer '+token})
  r=c.getresponse(); data=r.read(4097); value=json.loads(data) if len(data)<=4096 else None
  result['update_status']='passed' if r.status==200 and isinstance(value,dict) and value.get('state') in ('checking','up_to_date','downloading','waiting_for_idle','backing_up','installing','updating_gateway','completed') else 'unverified'
  c.close()
  if result['vm_services']=='passed' and result['update_status']=='passed':break
 except Exception:pass
 time.sleep(2)
print(json.dumps(result,separators=(',',':')))
`;

function valid(projectId, id, protectedInstallationIds = []) {
  if (!/^[a-z][a-z0-9-]{4,28}[a-z0-9]$/.test(projectId ?? '') || !/^[a-f0-9]{24}$/.test(id ?? '') || !Array.isArray(protectedInstallationIds) || protectedInstallationIds.some(value => !/^[a-f0-9]{12}([a-f0-9]{12})?$/.test(value) || id.startsWith(value.slice(0, 12)))) throw new Error('invalid_scope');
}

export function gcloudCommand(spawn = spawnSync, platform = process.platform) {
  if (platform !== 'win32') return { program: 'gcloud', prefix: [] };
  const found = spawn('where.exe', ['gcloud.cmd'], { encoding: 'utf8', windowsHide: true, timeout: 10000 });
  const command = found.status === 0 && found.stdout?.split(/\r?\n/).find(Boolean);
  if (!command || !/gcloud\.cmd$/i.test(command)) throw new Error('gcloud_unavailable');
  const root = resolve(dirname(command), '..');
  return { program: join(root, 'platform', 'bundledpython', 'python.exe'), prefix: ['-S', join(root, 'lib', 'gcloud.py')] };
}

export function sshProbe({ account, projectId, installationId, protectedInstallationIds = [] }, { spawn = spawnSync, platform = process.platform } = {}) {
  valid(projectId, installationId, protectedInstallationIds);
  if (!/^[A-Za-z0-9._%+@-]+$/.test(account ?? '')) throw new Error('invalid_account');
  const root = resolve(tmpdir()), directory = resolve(mkdtempSync(join(root, 'momso-s117c-')));
  if (!directory.startsWith(root + sep)) throw new Error('invalid_temp');
  try {
    const { program, prefix } = gcloudCommand(spawn, platform);
    const code = Buffer.from(remote, 'utf8').toString('base64');
    const wrapper = `import base64,subprocess; p=subprocess.run(['sudo','python3','-c',base64.b64decode('${code}').decode()],capture_output=True,timeout=120); print(p.stdout.decode() if p.returncode==0 else '{}')`;
    const args = [...prefix, 'compute', 'ssh', `momso-${installationId.slice(0, 12)}`, `--zone=${zone}`, `--project=${projectId}`, `--account=${account}`, '--tunnel-through-iap', '--strict-host-key-checking=no', `--ssh-key-file=${join(directory, 'ssh')}`, '--quiet', `--command=python3 -c "${wrapper}"`];
    const env = { ...process.env, CLOUDSDK_CORE_LOG_HTTP: 'false', CLOUDSDK_CORE_VERBOSITY: 'none', CLOUDSDK_CORE_DISABLE_PROMPTS: '1' };
    const reply = spawn(program, args, { encoding: 'utf8', windowsHide: true, timeout: 240000, maxBuffer: 65536, env });
    if (reply.status !== 0 || typeof reply.stdout !== 'string') return { vm_services: 'unverified', update_status: 'unverified' };
    const data = JSON.parse(reply.stdout.trim().split(/\r?\n/).at(-1));
    return Object.fromEntries(['vm_services', 'update_status'].map(key => [key, data[key] === 'passed' ? 'passed' : 'unverified']));
  } catch { return { vm_services: 'unverified', update_status: 'unverified' }; }
  finally { rmSync(directory, { recursive: true, force: true }); }
}

export async function temporaryIapDiagnostics(context, { token, fetchImpl = fetch, ssh = sshProbe, sleep = ms => new Promise(resolve => setTimeout(resolve, ms)) }) {
  const { projectId, installationId } = context;
  valid(projectId, installationId, context.protectedInstallationIds ?? []);
  const short = installationId.slice(0, 12), instance = `momso-${short}`;
  const rule = `${instance}-s117c-iap`, marker = `momso-s117c-${installationId}`;
  const base = `https://compute.googleapis.com/compute/v1/projects/${projectId}/`, endpoint = `${base}global/firewalls/${rule}`;
  const result = { vm_services: 'unverified', update_status: 'unverified', diagnosticFirewall: 'unknown' };
  let creationAttempted = false, originalId;
  const request = async (url, options = {}) => {
    const response = await fetchImpl(url, { ...options, headers: { Authorization: `Bearer ${token}`, ...(options.body ? { 'Content-Type': 'application/json' } : {}) }, redirect: 'error', cache: 'no-store' });
    if (!response.ok) throw new Error('unavailable');
    return response.status === 204 ? {} : response.json();
  };
  const poll = async operation => {
    if (!/^[A-Za-z0-9._-]+$/.test(operation?.name ?? '')) throw new Error('invalid_operation');
    for (let n = 0; n < 120; n++) {
      const current = await request(`${base}global/operations/${operation.name}`);
      if (current.status === 'DONE') { if (current.error) throw new Error('operation_failed'); return; }
      await sleep(1000);
    }
    throw new Error('operation_timeout');
  };
  const owns = value => value?.name === rule && value.description === marker && value.direction === 'INGRESS' && !value.disabled && value.network?.endsWith('/' + instance + '-net') && JSON.stringify(value.sourceRanges) === JSON.stringify(['35.235.240.0/20']) && JSON.stringify(value.targetTags) === JSON.stringify([instance]) && JSON.stringify(value.allowed) === JSON.stringify([{ IPProtocol: 'tcp', ports: ['22'] }]);
  try {
    const vm = await request(`${base}zones/${zone}/instances/${instance}`);
    if (vm.name !== instance || vm.labels?.['momso-install-id'] !== installationId || !vm.tags?.items?.includes(instance) || vm.networkInterfaces?.length !== 1 || !vm.networkInterfaces[0].network?.endsWith('/' + instance + '-net') || vm.networkInterfaces[0].accessConfigs?.length || vm.networkInterfaces[0].ipv6AccessConfigs?.length) throw new Error('ownership_unconfirmed');
    const existing = await fetchImpl(endpoint, { headers: { Authorization: `Bearer ${token}` }, redirect: 'error', cache: 'no-store' });
    if (existing.status !== 404) throw new Error('collision_or_unavailable');
    creationAttempted = true;
    const operation = await request(`${base}global/firewalls`, { method: 'POST', body: JSON.stringify({ name: rule, description: marker, network: `${base}global/networks/${instance}-net`, direction: 'INGRESS', sourceRanges: ['35.235.240.0/20'], targetTags: [instance], allowed: [{ IPProtocol: 'tcp', ports: ['22'] }] }) });
    await poll(operation);
    const current = await request(endpoint);
    if (!owns(current)) throw new Error('ownership_unconfirmed');
    originalId = current.id;
    Object.assign(result, await ssh(context));
  } catch { /* do not expose remote errors or credentials */ }
  finally {
    if (creationAttempted) {
      try {
        const current = await request(endpoint);
        if (!owns(current) || (originalId && current.id !== originalId)) throw new Error('ownership_unconfirmed');
        await poll(await request(endpoint, { method: 'DELETE' }));
        result.diagnosticFirewall = 'removed';
      } catch { result.diagnosticFirewall = 'remaining_or_unknown'; result.diagnosticRule = rule; }
    }
  }
  return result;
}
