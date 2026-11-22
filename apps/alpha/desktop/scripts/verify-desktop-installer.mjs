import { execFile } from 'node:child_process';
import { createHash } from 'node:crypto';
import { access, mkdtemp, readFile, readdir, rm } from 'node:fs/promises';
import os from 'node:os';
import path from 'node:path';
import { promisify } from 'node:util';
import { fileURLToPath } from 'node:url';

const exec = promisify(execFile);

export function runtimeImports(dump) {
  return [...new Set([...dump.matchAll(/^\s*((?:vcruntime|msvcp|msvcr|concrt|vccorlib|vcomp)\d[\w.-]*\.dll)\s*$/gim)].map(match => match[1].toLowerCase()))];
}

export function missingRuntimeImports(imports, names) {
  const available = new Set(names.map(name => name.toLowerCase()));
  return imports.filter(name => !available.has(name.toLowerCase()));
}

async function filesBelow(root) {
  const files = [];
  async function walk(dir) {
    for (const entry of await readdir(dir, { withFileTypes: true })) {
      const full = path.join(dir, entry.name);
      if (entry.isDirectory()) await walk(full);
      else if (entry.isFile()) files.push(full);
    }
  }
  await walk(root);
  return files;
}

async function findDumpbin() {
  const vswhere = path.join(process.env['ProgramFiles(x86)'], 'Microsoft Visual Studio', 'Installer', 'vswhere.exe');
  const { stdout } = await exec(vswhere, ['-latest', '-products', '*', '-requires', 'Microsoft.VisualStudio.Component.VC.Tools.x86.x64', '-property', 'installationPath']);
  const root = path.join(stdout.trim(), 'VC', 'Tools', 'MSVC');
  const versions = (await readdir(root)).sort((a, b) => b.localeCompare(a, 'en', { numeric: true }));
  for (const version of versions) {
    const candidate = path.join(root, version, 'bin', 'Hostx64', 'x64', 'dumpbin.exe');
    try { await readFile(candidate); return candidate; } catch (error) { if (error.code !== 'ENOENT') throw error; }
  }
  throw new Error('MSVC dumpbin is required to verify installer dependencies');
}

async function hash(file) { return createHash('sha256').update(await readFile(file)).digest('hex'); }

export function sameBundledExecutable(original, bundled) {
  if (original.equals(bundled)) return true;
  // Tauri patches this marker while bundling NSIS and restores the build file.
  const unknown = Buffer.from('__TAURI_BUNDLE_TYPE_VAR_UNK');
  const nsis = Buffer.from('__TAURI_BUNDLE_TYPE_VAR_NSS');
  const at = original.indexOf(unknown);
  if (at < 0 || original.indexOf(unknown, at + 1) >= 0 || bundled.length !== original.length) return false;
  const expected = Buffer.from(original);
  nsis.copy(expected, at);
  return expected.equals(bundled);
}

async function findSevenZip() {
  for (const base of [process.env.ProgramFiles, process.env['ProgramFiles(x86)']].filter(Boolean)) {
    const candidate = path.join(base, '7-Zip', '7z.exe');
    try { await access(candidate); return candidate; } catch (error) { if (error.code !== 'ENOENT') throw error; }
  }
  return '7z'; // A portable copy can instead be provided on PATH.
}

export async function verifyInstaller(installer, builtExe, stagedNode, stagedCrt) {
  const tmp = await mkdtemp(path.join(os.tmpdir(), 'momso-installer-verify-'));
  try {
    // Windows GitHub runners provide 7-Zip. Extraction verifies the bytes in
    // the actual NSIS artifact, not only the input resource configuration.
    await exec(await findSevenZip(), ['x', '-y', `-o${tmp}`, installer], { maxBuffer: 8 * 1024 * 1024 });
    const dumpbin = await findDumpbin();
    await verifyExtracted(tmp, builtExe, stagedNode, stagedCrt, async binary => {
      const { stdout } = await exec(dumpbin, ['/DEPENDENTS', binary], { maxBuffer: 8 * 1024 * 1024 });
      return runtimeImports(stdout);
    });
    const { stdout } = await exec(dumpbin, ['/DEPENDENTS', installer], { maxBuffer: 8 * 1024 * 1024 });
    const missing = runtimeImports(stdout);
    if (missing.length) throw new Error(`Installer needs runtime DLLs before installation: ${missing.join(', ')}`);
  } finally { await rm(tmp, { recursive: true, force: true }); }
}

export async function verifyExtracted(tmp, builtExe, stagedNode, stagedCrt, inspectImports) {
  const files = await filesBelow(tmp);
  const unique = name => {
    const matches = files.filter(file => path.basename(file).toLowerCase() === name);
    if (matches.length !== 1) throw new Error(`Expected one ${name} in the installer; found ${matches.length}`);
    return matches[0];
  };
  const app = unique('momso-desktop.exe');
  const node = unique('node.exe');
  if (!sameBundledExecutable(await readFile(builtExe), await readFile(app)) || await hash(node) !== await hash(stagedNode)) throw new Error('Installer executable differs from the built executable');
  for (const name of await readdir(stagedCrt)) {
    const sourceHash = await hash(path.join(stagedCrt, name));
    for (const folder of [path.dirname(app), path.dirname(node)]) {
      if (await hash(path.join(folder, name)) !== sourceHash) throw new Error(`Installer runtime mismatch: ${name}`);
    }
  }
  // Check shipped executables, DLLs and Node native modules, including
  // transitive CRT imports and delay-load imports listed by dumpbin.
  const binaries = files.filter(file => /\.(?:exe|dll|node)$/i.test(file));
  for (const binary of binaries) {
    const missing = missingRuntimeImports(await inspectImports(binary), await readdir(path.dirname(binary)));
    if (missing.length) throw new Error(`${path.relative(tmp, binary)} is missing adjacent runtime DLLs: ${missing.join(', ')}`);
  }
  process.stdout.write(`Verified ${binaries.length} installer executables and runtime DLLs\n`);
}

if (process.argv[1] && path.resolve(process.argv[1]) === fileURLToPath(import.meta.url)) {
  const [installer, builtExe, stagedNode, stagedCrt] = process.argv.slice(2);
  if (!installer || !builtExe || !stagedNode || !stagedCrt) throw new Error('Expected installer, built executable, bundled Node, and runtime paths');
  verifyInstaller(installer, builtExe, stagedNode, stagedCrt).catch(error => { console.error(error.message); process.exitCode = 1; });
}
