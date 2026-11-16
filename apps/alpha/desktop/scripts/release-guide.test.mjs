import { test } from 'node:test';
import assert from 'node:assert/strict';
import { spawnSync } from 'node:child_process';
import { mkdtemp, mkdir, readFile, rm, writeFile } from 'node:fs/promises';
import os from 'node:os';
import path from 'node:path';
import { fileURLToPath } from 'node:url';

const desktop = path.resolve(path.dirname(fileURLToPath(import.meta.url)), '..');
const guide = await readFile(path.join(desktop, 'RELEASE.md'), 'utf8');
const blocks = [...guide.matchAll(/```powershell\r?\n([\s\S]*?)\r?\n```/g)];
assert.equal(blocks.length, 2);

async function fixture(signature, run) {
  const dir = await mkdtemp(path.join(os.tmpdir(), 'momso-release-test-'));
  try {
    const configDir = path.join(dir, 'apps/alpha/desktop/src-tauri');
    const bundle = path.join(configDir, 'target/release/bundle/nsis');
    await mkdir(bundle, { recursive: true });
    const config = JSON.parse(await readFile(path.join(desktop, 'src-tauri/tauri.conf.json'), 'utf8'));
    config.version = '1.2.3';
    config.plugins.updater.endpoints = ['https://github.com/example/momso/releases/latest/download/latest.json'];
    await writeFile(path.join(configDir, 'tauri.conf.json'), JSON.stringify(config));
    await writeFile(path.join(bundle, 'momso_1.2.3_x64-setup.exe'), 'fixture');
    await writeFile(path.join(bundle, 'momso_1.2.3_x64-setup.exe.sig'), signature);
    await run(dir, bundle);
  } finally {
    await rm(dir, { recursive: true, force: true });
  }
}

function runGuide(dir) {
  const ps = `$ErrorActionPreference='Stop'; $script:i=0; function Read-Host { param($Prompt) $script:i++; if($script:i -eq 1){'1.2.3'}else{'v1.2.3'} }; ${blocks[1][1]}`;
  return spawnSync('powershell.exe', ['-NoProfile', '-NonInteractive', '-EncodedCommand', Buffer.from(ps, 'utf16le').toString('base64')], { cwd: dir, encoding: 'utf8' });
}

test('PowerShell 5.1 guide builds updater metadata from the configured endpoint', async () => {
  await fixture('fixture signature\n', async (dir, bundle) => {
    const result = runGuide(dir);
    assert.equal(result.status, 0, result.stderr);
    const bytes = await readFile(path.join(bundle, 'latest.json'));
    assert.notDeepEqual([...bytes.subarray(0, 3)], [0xef, 0xbb, 0xbf]);
    const json = JSON.parse(bytes.toString('utf8'));
    assert.equal(json.version, '1.2.3');
    assert.equal(json.platforms['windows-x86_64'].signature, 'fixture signature\n');
    assert.equal(json.platforms['windows-x86_64'].url, 'https://github.com/example/momso/releases/download/v1.2.3/momso_1.2.3_x64-setup.exe');
  });
});

test('PowerShell 5.1 guide refuses an empty signature', async () => {
  await fixture('', async (dir) => {
    assert.notEqual(runGuide(dir).status, 0);
  });
});
