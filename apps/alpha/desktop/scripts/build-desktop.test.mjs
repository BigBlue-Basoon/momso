import test from 'node:test';
import assert from 'node:assert/strict';
import { longestPath, omitNodeFile } from './build-desktop.mjs';

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
