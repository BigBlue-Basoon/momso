import { test } from 'node:test';
import assert from 'node:assert/strict';
import { readFile } from 'node:fs/promises';
import path from 'node:path';
import { fileURLToPath } from 'node:url';

const root = path.resolve(path.dirname(fileURLToPath(import.meta.url)), '..');

test('application bundle does not include generated MinGit', async () => {
  const config = JSON.parse(await readFile(path.join(root, 'src-tauri/tauri.conf.json'), 'utf8'));
  const resources = JSON.stringify(config.bundle.resources ?? {});
  assert.doesNotMatch(resources, /mingit/i);
  const entries = config.bundle.resources ?? {};
  for (const source of Array.isArray(entries) ? entries : Object.keys(entries)) {
    assert.doesNotMatch(source.replaceAll('\\', '/').replace(/\/$/, ''), /(^|\/)generated$/i);
  }
  assert.match(resources, /third-party/);
  assert.equal(entries['generated/node/'], 'node/');
  await readFile(path.join(root, 'src-tauri/generated/node/.gitkeep'));
});

test('normal application build does not download MinGit', async () => {
  const pkg = JSON.parse(await readFile(path.join(root, 'package.json'), 'utf8'));
  assert.doesNotMatch(pkg.scripts['build:bundle'], /prepare-mingit|download|mingit/i);
  assert.match(pkg.scripts['build:bundle'], /prepare-node/);
});
