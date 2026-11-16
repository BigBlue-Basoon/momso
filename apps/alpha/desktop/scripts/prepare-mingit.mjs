import { createHash } from 'node:crypto';
import { lstat, mkdir, mkdtemp, readFile, readdir, rename, rm, stat, writeFile } from 'node:fs/promises';
import os from 'node:os';
import path from 'node:path';
import { execFile } from 'node:child_process';
import { promisify } from 'node:util';
import { fileURLToPath } from 'node:url';
import { unzipSync } from 'fflate';

// Git for Windows release asset digest, published alongside the pinned asset.
export const RELEASE = Object.freeze({
  version: '2.56.1',
  url: 'https://github.com/git-for-windows/git/releases/download/v2.56.1.windows.1/MinGit-2.56.1-64-bit.zip',
  sha256: '4a92850b6e917da4fd010669b614b55dccf5b0ec333a534cc4be4700dfefdafd',
});
const here = path.dirname(fileURLToPath(import.meta.url));
const generated = path.resolve(here, '../src-tauri/generated');

export function decodeVerifiedArchive(data, expectedHash) {
  const actual = createHash('sha256').update(data).digest('hex');
  if (actual !== expectedHash) throw new Error('MinGit SHA-256 mismatch');
  const entries = unzipSync(data);
  const files = new Map();
  for (const [name, bytes] of Object.entries(entries)) {
    if (name.includes('\\') || name.startsWith('/') || name.includes(':') ||
        name.split('/').some((part) => part === '..' || part === '.')) {
      throw new Error('Unsafe archive entry');
    }
    if (!name.endsWith('/')) files.set(name, bytes);
  }
  if (!files.has('cmd/git.exe')) throw new Error('MinGit executable missing');
  return files;
}

async function listFiles(dir, base = dir) {
  const found = [];
  for (const entry of await readdir(dir, { withFileTypes: true })) {
    const full = path.join(dir, entry.name);
    if (entry.isSymbolicLink()) throw new Error('Unexpected link in generated MinGit');
    if (entry.isDirectory()) found.push(...await listFiles(full, base));
    else if (entry.isFile()) found.push(path.relative(base, full).split(path.sep).join('/'));
    else throw new Error('Unexpected entry in generated MinGit');
  }
  return found;
}

export async function verifyExisting(dir, files) {
  const actual = await listFiles(dir);
  if (actual.length !== files.size) throw new Error('Existing generated MinGit differs from verified release');
  for (const name of actual) {
    const expected = files.get(name);
    if (!expected || !Buffer.from(expected).equals(await readFile(path.join(dir, ...name.split('/'))))) {
      throw new Error('Existing generated MinGit differs from verified release');
    }
  }
}

export async function prepare(data, targetParent = generated) {
  const files = decodeVerifiedArchive(data, RELEASE.sha256);
  await mkdir(targetParent, { recursive: true });
  const target = path.join(targetParent, 'mingit');
  try {
    const existing = await lstat(target);
    if (!existing.isDirectory() || existing.isSymbolicLink()) throw new Error('Invalid generated MinGit directory');
    await verifyExisting(target, files);
    return target;
  } catch (error) {
    if (error.code !== 'ENOENT') throw error;
  }
  const stage = await mkdtemp(path.join(targetParent, '.mingit-stage-'));
  try {
    for (const [name, bytes] of files) {
      const dest = path.join(stage, ...name.split('/'));
      await mkdir(path.dirname(dest), { recursive: true });
      await writeFile(dest, bytes, { flag: 'wx' });
    }
    await verifyExisting(stage, files);
    await rename(stage, target);
    return target;
  } finally {
    await rm(stage, { recursive: true, force: true });
  }
}

async function main() {
  if (process.platform !== 'win32' || process.arch !== 'x64') {
    throw new Error('This MinGit bundle is pinned for Windows x64');
  }
  const temp = await mkdtemp(path.join(os.tmpdir(), 'momso-mingit-download-'));
  try {
    const archive = path.join(temp, 'mingit.zip');
    await promisify(execFile)('powershell.exe', [
      '-NoProfile', '-NonInteractive', '-Command',
      "$ErrorActionPreference='Stop'; $ProgressPreference='SilentlyContinue'; Invoke-WebRequest -Uri $env:MOMSO_MINGIT_URL -OutFile $env:MOMSO_MINGIT_DOWNLOAD -UseBasicParsing",
    ], { timeout: 180000, env: { ...process.env, MOMSO_MINGIT_URL: RELEASE.url, MOMSO_MINGIT_DOWNLOAD: archive } });
    if ((await stat(archive)).size > 100_000_000) throw new Error('MinGit archive exceeds size limit');
    await prepare(new Uint8Array(await readFile(archive)));
  } finally {
    await rm(temp, { recursive: true, force: true });
  }
  process.stdout.write(`Verified MinGit ${RELEASE.version}\n`);
}

if (process.argv[1] && path.resolve(process.argv[1]) === fileURLToPath(import.meta.url)) {
  main().catch((error) => { process.stderr.write(`${error.message}\n`); process.exitCode = 1; });
}
