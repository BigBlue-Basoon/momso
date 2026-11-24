import test from 'node:test';
import assert from 'node:assert/strict';
import { mkdir, mkdtemp, readdir, rm, writeFile } from 'node:fs/promises';
import os from 'node:os';
import path from 'node:path';
import { longestPath, omitNodeFile, releaseBuildEnvironment, selectStageParent, withStage } from './build-desktop.mjs';
import { missingRuntimeImports, runtimeImports, sameBundledExecutable, verifyExtracted } from './verify-desktop-installer.mjs';

test('remaps long and short profile paths in Rust and C builds', () => {
  const long = 'C:\\Profiles\\fixture person';
  const short = 'C:\\Profiles\\FIXTUR~1';
  const env = releaseBuildEnvironment({ RUSTFLAGS: '-C debuginfo=0' }, [long, short]);
  assert.ok(env.CARGO_ENCODED_RUSTFLAGS.split('\x1f').includes(`${long}=C:\\momso-build`));
  assert.ok(env.CL.includes(`"/pathmap:${short}=C:\\momso-build"`));
});

test('accepts only the Tauri NSIS marker change in the app binary', () => {
  const before = Buffer.from('prefix__TAURI_BUNDLE_TYPE_VAR_UNKsuffix');
  assert.equal(sameBundledExecutable(before, Buffer.from('prefix__TAURI_BUNDLE_TYPE_VAR_NSSsuffix')), true);
  assert.equal(sameBundledExecutable(before, Buffer.from('prefix__TAURI_BUNDLE_TYPE_VAR_NSSsuffiy')), false);
  assert.equal(sameBundledExecutable(before, Buffer.from('prefix__TAURI_BUNDLE_TYPE_VAR_MSIsuffix')), false);
});

test('detects missing adjacent Visual C++ runtime dependencies case insensitively', () => {
  const imports = runtimeImports('  VCRUNTIME140.dll\r\n  VCRUNTIME140_1.dll\r\n  MSVCP140.dll\r\n  KERNEL32.dll\r\n');
  assert.deepEqual(imports, ['vcruntime140.dll', 'vcruntime140_1.dll', 'msvcp140.dll']);
  assert.deepEqual(missingRuntimeImports(imports, ['vcruntime140.dll', 'MSVCP140.dll']), ['vcruntime140_1.dll']);
  assert.deepEqual(missingRuntimeImports(imports, ['VCRUNTIME140.DLL', 'VCRUNTIME140_1.DLL', 'msvcp140.dll']), []);
});

test('checks extracted installer bytes and rejects a missing runtime next to either executable', async () => {
  const root = await mkdtemp(path.join(os.tmpdir(), 'momso-runtime-test-'));
  try {
    const extracted = path.join(root, 'extracted');
    const nodeDir = path.join(extracted, 'node');
    const crt = path.join(root, 'crt');
    await mkdir(nodeDir, { recursive: true });
    await mkdir(crt);
    const app = path.join(root, 'app.exe');
    const node = path.join(root, 'node.exe');
    await writeFile(app, 'app');
    await writeFile(node, 'node');
    await writeFile(path.join(extracted, 'momso-desktop.exe'), 'app');
    await writeFile(path.join(nodeDir, 'node.exe'), 'node');
    await writeFile(path.join(crt, 'vcruntime140.dll'), 'microsoft-runtime');
    await writeFile(path.join(extracted, 'vcruntime140.dll'), 'microsoft-runtime');
    await writeFile(path.join(nodeDir, 'vcruntime140.dll'), 'microsoft-runtime');
    const imports = async binary => binary.endsWith('.exe') ? ['vcruntime140.dll'] : [];
    await verifyExtracted(extracted, app, node, crt, imports);
    await rm(path.join(nodeDir, 'vcruntime140.dll'));
    await assert.rejects(verifyExtracted(extracted, app, node, crt, imports), /ENOENT/);
    await writeFile(path.join(nodeDir, 'vcruntime140.dll'), 'altered');
    await assert.rejects(verifyExtracted(extracted, app, node, crt, imports), /runtime mismatch/);
  } finally { await rm(root, { recursive: true, force: true }); }
});

test('omits unused Corepack and documentation without removing npm or licenses', () => {
  assert.equal(omitNodeFile('node_modules/corepack/dist/corepack.js'), true);
  assert.equal(omitNodeFile('node_modules/npm/README.md'), true);
  assert.equal(omitNodeFile('node_modules/npm/docs/content/commands/npm-install.md'), true);
  assert.equal(omitNodeFile('node_modules/npm/LICENSE'), false);
  assert.equal(omitNodeFile('node_modules/npm/bin/npm-cli.js'), false);
  assert.equal(omitNodeFile('node.exe'), false);
});

test('reports the longest fully qualified staged source path', () => {
  const result = longestPath('C:\\Temp\\momso', ['node.exe', 'node_modules/npm/bin/npm-cli.js']);
  assert.ok(result.endsWith('npm-cli.js'));
  assert.ok(result.length > 'C:\\Temp\\momso\\node.exe'.length);
});

test('uses the build drive when the temporary directory is on another drive', () => {
  assert.equal(selectStageParent('C:\\Temp\\elsewhere', 'D:\\a\\momso\\src-tauri', path.win32), 'D:\\a\\momso\\src-tauri\\target');
  assert.equal(selectStageParent('C:\\Temp', 'c:\\work\\src-tauri', path.win32), 'C:\\Temp');
  assert.throws(() => selectStageParent('relative', 'D:\\work', path.win32), /Absolute/);
});

test('removes staged files after success and failure', async () => {
  const root = await mkdtemp(path.join(os.tmpdir(), 'momso-stage-test-'));
  try {
    await withStage(root, async stage => { await writeFile(path.join(stage, 'fixture'), 'test'); });
    assert.deepEqual(await readdir(root), []);
    await assert.rejects(withStage(root, async stage => {
      await writeFile(path.join(stage, 'fixture'), 'test');
      throw new Error('fixture failure');
    }), /fixture failure/);
    assert.deepEqual(await readdir(root), []);
  } finally {
    await rm(root, { recursive: true, force: true });
  }
});
