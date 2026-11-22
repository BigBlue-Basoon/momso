import { createHash } from 'node:crypto';
import { cp, mkdir, mkdtemp, readFile, readdir, realpath, rm } from 'node:fs/promises';
import { spawn, execFile } from 'node:child_process';
import { promisify } from 'node:util';
import os from 'node:os';
import path from 'node:path';
import { fileURLToPath } from 'node:url';

const desktop = path.resolve(path.dirname(fileURLToPath(import.meta.url)), '..');
const tauri = path.join(desktop, 'src-tauri');
const nodeSource = path.join(tauri, 'generated', 'node');
const noticeSource = path.join(desktop, 'third-party');
const cli = path.resolve(desktop, '../../../node_modules/@tauri-apps/cli/tauri.js');
const MAX_SOURCE_PATH = 259;

export function omitNodeFile(relative) {
  const parts = relative.replaceAll('\\', '/').split('/');
  if (parts[0] === 'node_modules' && parts[1] === 'corepack') return true;
  if (parts[0] === 'node_modules' && parts[1] === 'npm' && ['docs', 'html', 'man'].includes(parts[2])) return true;
  return /^(?:readme|changelog|history|contributing)(?:\.[a-z0-9_-]+)?$/i.test(parts.at(-1));
}

export async function filesBelow(root, dir = root) {
  const result = [];
  for (const entry of await readdir(dir, { withFileTypes: true })) {
    const full = path.join(dir, entry.name);
    if (entry.isSymbolicLink()) throw new Error('Unexpected link in bundled resources');
    if (entry.isDirectory()) result.push(...await filesBelow(root, full));
    else if (entry.isFile()) result.push(path.relative(root, full));
    else throw new Error('Unexpected bundled resource entry');
  }
  return result;
}

export function longestPath(root, files) {
  return files.reduce((longest, item) => {
    const full = path.join(root, item);
    return full.length > longest.length ? full : longest;
  }, '');
}

export function selectStageParent(tempRoot, tauriRoot, pathApi = path) {
  const tempDrive = pathApi.parse(tempRoot).root.toLowerCase();
  const tauriDrive = pathApi.parse(tauriRoot).root.toLowerCase();
  if (!tempDrive || !tauriDrive) throw new Error('Absolute build paths required');
  return tempDrive === tauriDrive ? tempRoot : pathApi.join(tauriRoot, 'target');
}

// Use the redistributable CRT shipped with the same Visual Studio installation
// that supplies the MSVC linker. System32 is not a redistributable source.
export async function locateMsvcCrt() {
  const vswhere = path.join(process.env['ProgramFiles(x86)'], 'Microsoft Visual Studio', 'Installer', 'vswhere.exe');
  const { stdout } = await promisify(execFile)(vswhere, ['-latest', '-products', '*', '-requires', 'Microsoft.VisualStudio.Component.VC.Tools.x86.x64', '-property', 'installationPath']);
  const root = path.join(stdout.trim(), 'VC', 'Redist', 'MSVC');
  const versions = (await readdir(root, { withFileTypes: true }))
    .filter(entry => entry.isDirectory() && /^\d+(?:\.\d+)+$/.test(entry.name))
    .map(entry => entry.name).sort((a, b) => b.localeCompare(a, 'en', { numeric: true }));
  for (const version of versions) {
    const arch = path.join(root, version, 'x64');
    try {
      const families = (await readdir(arch, { withFileTypes: true }))
        .filter(entry => entry.isDirectory() && /^Microsoft\.VC\d+\.CRT$/i.test(entry.name))
        .map(entry => entry.name).sort((a, b) => b.localeCompare(a, 'en', { numeric: true }));
      for (const family of families) {
        const folder = path.join(arch, family);
        const names = (await readdir(folder)).filter(name => /^(?:msvcp|vcruntime|concrt|vccorlib)\d.*\.dll$/i.test(name));
        if (['vcruntime140.dll', 'vcruntime140_1.dll', 'msvcp140.dll'].every(name => names.some(actual => actual.toLowerCase() === name))) return { folder, names };
      }
    } catch (error) { if (error.code !== 'ENOENT') throw error; }
  }
  throw new Error('Visual C++ x64 redistributable files are required to build the installer');
}

export async function withStage(parent, action) {
  const stage = await mkdtemp(path.join(parent, 'momso-bundle-'));
  try {
    if (path.dirname(stage) !== parent) throw new Error('Invalid temporary bundle path');
    return await action(stage);
  } finally {
    try { await rm(stage, { recursive: true, force: true }); }
    catch (error) { process.stderr.write(`Could not remove build resources: ${stage}\n`); throw error; }
  }
}

function run(command, args, options = {}) {
  return new Promise((resolve, reject) => {
    const child = spawn(command, args, { cwd: desktop, stdio: 'inherit', ...options });
    child.once('error', reject);
    child.once('exit', (code, signal) => code === 0 ? resolve() : reject(new Error(`${path.basename(command)} failed: ${signal || code}`)));
  });
}

async function digest(file) {
  return createHash('sha256').update(await readFile(file)).digest('hex');
}

async function main() {
  if (process.platform !== 'win32' || process.arch !== 'x64') throw new Error('Windows x64 required');
  const args = process.argv.slice(2);
  if (args.some(arg => arg !== '--updater') || args.length > 1) throw new Error('Supported option: --updater');
  await run(process.execPath, [path.join(desktop, 'scripts/prepare-node.mjs')]);
  const tempRoot = await realpath(os.tmpdir());
  const tauriRoot = await realpath(tauri);
  const chosenParent = selectStageParent(tempRoot, tauriRoot);
  await mkdir(chosenParent, { recursive: true });
  const stageParent = await realpath(chosenParent);
  if (path.parse(stageParent).root.toLowerCase() !== path.parse(tauriRoot).root.toLowerCase()) {
    throw new Error('Bundle staging must be on the build drive');
  }
  await withStage(stageParent, async stage => {
    const node = path.join(stage, 'node');
    const notices = path.join(stage, 'third-party');
    await cp(nodeSource, node, { recursive: true, filter: source => !omitNodeFile(path.relative(nodeSource, source)) });
    await cp(noticeSource, notices, { recursive: true });
    const crt = await locateMsvcCrt();
    await promisify(execFile)('powershell.exe', ['-NoProfile', '-NonInteractive', '-Command',
      "$ErrorActionPreference='Stop'; Import-Module (Join-Path $env:SystemRoot 'System32\\WindowsPowerShell\\v1.0\\Modules\\Microsoft.PowerShell.Security\\Microsoft.PowerShell.Security.psd1'); foreach ($name in (ConvertFrom-Json $env:MOMSO_CRT_NAMES)) { $sig = Get-AuthenticodeSignature -LiteralPath (Join-Path $env:MOMSO_CRT_SOURCE $name); if ($sig.Status -ne 'Valid' -or $sig.SignerCertificate.Subject -notmatch 'O=Microsoft Corporation') { throw 'Invalid Microsoft runtime signature' } }"],
    { env: { ...process.env, MOMSO_CRT_SOURCE: crt.folder, MOMSO_CRT_NAMES: JSON.stringify(crt.names) } });
    const crtStage = path.join(stage, 'crt');
    await mkdir(crtStage);
    for (const name of crt.names) {
      await cp(path.join(crt.folder, name), path.join(crtStage, name));
      await cp(path.join(crt.folder, name), path.join(node, name));
    }
    const nodeFiles = await filesBelow(node);
    const noticeFiles = await filesBelow(notices);
    for (const required of ['node.exe', path.join('node_modules', 'npm', 'bin', 'npm-cli.js')]) {
      if (!nodeFiles.includes(required)) throw new Error(`Missing Node runtime file: ${required}`);
    }
    for (const file of nodeFiles.filter(file => !crt.names.includes(file))) {
      if (await digest(path.join(node, file)) !== await digest(path.join(nodeSource, file))) throw new Error(`Node staging mismatch: ${file}`);
    }
    for (const file of noticeFiles) {
      if (await digest(path.join(notices, file)) !== await digest(path.join(noticeSource, file))) throw new Error(`Notice staging mismatch: ${file}`);
    }
    const longest = [longestPath(node, nodeFiles), longestPath(notices, noticeFiles), path.join(tauri, 'target/release/nsis/x64/installer.nsi')]
      .reduce((a, b) => a.length >= b.length ? a : b);
    process.stdout.write(`Longest bundle source path: ${longest.length} characters (limit ${MAX_SOURCE_PATH})\n`);
    if (longest.length > MAX_SOURCE_PATH) throw new Error('Bundle path is too long for Windows NSIS');
    const resources = { 'generated/node/': null, '../third-party/': null };
    resources[`${node.replaceAll('\\', '/')}/`] = 'node/';
    resources[`${notices.replaceAll('\\', '/')}/`] = 'third-party/';
    for (const name of crt.names) resources[path.join(crtStage, name).replaceAll('\\', '/')] = name;
    const config = { build: { beforeBuildCommand: 'npm run build' }, bundle: { resources } };
    if (args.includes('--updater')) config.bundle.createUpdaterArtifacts = true;
    await run(process.execPath, [cli, 'build', '--bundles', 'nsis', '--config', JSON.stringify(config)]);
    const { version } = JSON.parse(await readFile(path.join(tauri, 'tauri.conf.json'), 'utf8'));
    const target = process.env.CARGO_TARGET_DIR ? path.resolve(process.env.CARGO_TARGET_DIR) : path.join(tauri, 'target');
    const release = path.join(target, 'release');
    const installer = path.join(release, 'bundle', 'nsis', `momso_${version}_x64-setup.exe`);
    await run(process.execPath, [path.join(desktop, 'scripts/verify-desktop-installer.mjs'), installer, path.join(release, 'momso-desktop.exe'), path.join(node, 'node.exe'), crtStage]);
  });
}

if (process.argv[1] && path.resolve(process.argv[1]) === fileURLToPath(import.meta.url)) {
  main().catch(error => { console.error(error.message); process.exitCode = 1; });
}
