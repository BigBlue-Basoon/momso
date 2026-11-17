import test from 'node:test';
import assert from 'node:assert/strict';
import { mkdtemp, readdir, rm, writeFile } from 'node:fs/promises';
import os from 'node:os';
import path from 'node:path';
import { longestPath, omitNodeFile, selectStageParent, withStage } from './build-desktop.mjs';

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
  assert.equal(selectStageParent('C:\\Users\\runner\\Temp', 'D:\\a\\momso\\src-tauri', path.win32), 'D:\\a\\momso\\src-tauri\\target');
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
