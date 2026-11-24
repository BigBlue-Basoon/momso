import { inspectProjectExclusivity } from './momso_anywhere_project_exclusivity.mjs';

let data = '';
try {
  for await (const part of process.stdin) { data += part; if (data.length > 25000000) throw new Error('invalid'); }
  const { projectId, installationId, assets, services } = JSON.parse(data);
  const list = value => { if (!Array.isArray(value)) throw new Error('unavailable'); return value; };
  console.log(JSON.stringify(await inspectProjectExclusivity({ projectId, installationId, listAssets: async () => list(assets), listServices: async () => list(services) })));
} catch { process.stderr.write('inventory_unavailable\n'); process.exitCode = 1; }
