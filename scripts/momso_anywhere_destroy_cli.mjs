import { readFileSync } from 'node:fs';
import { lstat, mkdtemp, readFile, rename, rm, writeFile } from 'node:fs/promises';
import { dirname, join, resolve } from 'node:path';
import { pathToFileURL } from 'node:url';
import { runDestroyCommand } from './momso_anywhere_destroy_command.mjs';

export async function executeDestroy({ projectId, installationId, token, path, providerFactory, onProgress = () => {} }) {
  const target = resolve(path);
  try {
    const info = await lstat(target);
    if (!info.isFile() || info.isSymbolicLink()) throw new Error('state_unavailable');
    const previous = JSON.parse(await readFile(target, 'utf8'));
    if (previous.projectId !== projectId || previous.installationId !== installationId) throw new Error('state_unavailable');
  } catch (error) { if (error.code !== 'ENOENT') throw error; }
  return runDestroyCommand({ projectId, installationId, token, providerFactory, onProgress, saveState: async value => {
    const directory = await mkdtemp(join(dirname(target), '.momso-destroy-'));
    try { const temporary = join(directory, 'state.json'); await writeFile(temporary, JSON.stringify(value) + '\n', { mode: 0o600 }); await rename(temporary, target); }
    finally { await rm(directory, { recursive: true, force: true }); }
  } });
}

async function main() {
const names = ['--project', '--installation-id', '--state'];
const args = process.argv.slice(2);
const values = {};
for (let i = 0; i < args.length; i += 2) {
  if (!names.includes(args[i]) || !args[i + 1] || values[args[i]]) { process.stderr.write('invalid_arguments\n'); process.exit(1); }
  values[args[i]] = args[i + 1];
}
if (names.some(name => !values[name])) { process.stderr.write('invalid_arguments\n'); process.exit(1); }
const token = readFileSync(0, 'utf8').trim();
if (!token || /\s/.test(token)) { process.stderr.write('invalid_authorization\n'); process.exit(1); }
const projectId = values['--project'];
const installationId = values['--installation-id'];
const path = values['--state'];
try {
  const result = await executeDestroy({ projectId, installationId, token, path, onProgress: progress => process.stdout.write(JSON.stringify(progress) + '\n') });
  process.stdout.write(JSON.stringify(result) + '\n');
  if (result.remaining.length) process.exitCode = 2;
} catch (error) {
  process.stderr.write(`${typeof error?.step === 'string' ? error.step : 'destroy_failed'}\n`);
  process.exitCode = 1;
}
}

if (process.argv[1] && import.meta.url === pathToFileURL(resolve(process.argv[1])).href) await main();
