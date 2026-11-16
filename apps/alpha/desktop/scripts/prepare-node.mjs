import { createHash } from 'node:crypto';
import { lstat, mkdir, mkdtemp, readFile, readdir, rename, rm, stat, writeFile } from 'node:fs/promises';
import { execFile } from 'node:child_process';
import { promisify } from 'node:util';
import { fileURLToPath } from 'node:url';
import os from 'node:os';
import path from 'node:path';
import { unzipSync } from 'fflate';

// SHA-256 from https://nodejs.org/dist/v22.23.5/SHASUMS256.txt.
const VERSION = '22.23.5';
const DIGEST = '4a191082e9706866830379d577d2cc8a50c776fcd9436a8d061b063ed0d9a0e3';
const generated = path.resolve(path.dirname(fileURLToPath(import.meta.url)), '../src-tauri/generated');
const target = path.join(generated, 'node');
async function allFiles(dir, base = dir) {
  const names = [];
  for (const entry of await readdir(dir, { withFileTypes: true })) {
    const full = path.join(dir, entry.name);
    if (entry.isSymbolicLink()) throw new Error('Unexpected link in Node runtime');
    if (entry.isDirectory()) names.push(...await allFiles(full, base));
    else if (entry.isFile()) names.push(path.relative(base, full).split(path.sep).join('/'));
    else throw new Error('Unexpected Node runtime entry');
  }
  return names;
}
const tmp = await mkdtemp(path.join(os.tmpdir(), 'momso-node-download-'));
try {
  if (process.platform !== 'win32' || process.arch !== 'x64') throw new Error('Windows x64 required');
  const zip = path.join(tmp, 'node.zip');
  await promisify(execFile)('powershell.exe', ['-NoProfile', '-NonInteractive', '-Command',
    "$ErrorActionPreference='Stop'; $ProgressPreference='SilentlyContinue'; Invoke-WebRequest -Uri $env:MOMSO_NODE_URL -OutFile $env:MOMSO_NODE_ZIP -UseBasicParsing"],
  { timeout: 180000, env: { ...process.env, MOMSO_NODE_URL: `https://nodejs.org/dist/v${VERSION}/node-v${VERSION}-win-x64.zip`, MOMSO_NODE_ZIP: zip } });
  if ((await stat(zip)).size > 100_000_000) throw new Error('Node archive exceeds size limit');
  const bytes = await readFile(zip);
  if (createHash('sha256').update(bytes).digest('hex') !== DIGEST) throw new Error('Node checksum mismatch');
  const entries = unzipSync(bytes);
  const prefix = `node-v${VERSION}-win-x64/`;
  const files = new Map();
  for (const [name, content] of Object.entries(entries)) {
    if (!name.startsWith(prefix)) throw new Error('Unexpected Node archive entry');
    const relative = name.slice(prefix.length);
    if (relative && (relative.includes('\\') || relative.includes(':') || relative.split('/').some(p => p === '..' || p === '.'))) throw new Error('Unsafe Node archive entry');
    if (relative && !relative.endsWith('/')) files.set(relative, content);
  }
  if (!files.has('node.exe') || !files.has('node_modules/npm/bin/npm-cli.js')) throw new Error('Node or npm missing');
  await mkdir(generated, { recursive: true });
  try {
    const existing = await lstat(target);
    if (!existing.isDirectory() || existing.isSymbolicLink()) throw new Error('Invalid Node directory');
    if ((await allFiles(target)).length !== files.size) throw new Error('Existing Node differs from verified archive');
    for (const [name, content] of files) {
      const actual = await readFile(path.join(target, ...name.split('/')));
      if (!actual.equals(Buffer.from(content))) throw new Error('Existing Node differs from verified archive');
    }
  } catch (error) {
    if (error.code !== 'ENOENT') throw error;
    const stage = await mkdtemp(path.join(generated, '.node-stage-'));
    try {
      for (const [name, content] of files) {
        const out = path.join(stage, ...name.split('/'));
        await mkdir(path.dirname(out), { recursive: true });
        await writeFile(out, content, { flag: 'wx' });
      }
      await rename(stage, target);
    } finally { await rm(stage, { recursive: true, force: true }); }
  }
  process.stdout.write(`Verified Node.js ${VERSION}\n`);
} finally { await rm(tmp, { recursive: true, force: true }); }
