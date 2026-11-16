import { spawnSync } from 'node:child_process';
import { mkdtemp, readFile, realpath, rm, writeFile } from 'node:fs/promises';
import os from 'node:os';
import path from 'node:path';
import { fileURLToPath } from 'node:url';

const desktop = path.resolve(path.dirname(fileURLToPath(import.meta.url)), '..');
const cli = path.resolve(desktop, '../../../node_modules/@tauri-apps/cli/tauri.js');
const base = await realpath(os.tmpdir());
const dir = await mkdtemp(path.join(base, 'momso-updater-test-'));
if (path.dirname(dir) !== base) throw new Error('Invalid temporary test path');
const key = path.join(dir, 'test.key');
const payload = path.join(dir, 'update.bin');
const other = path.join(dir, 'other.bin');
const password = 'temporary-test-key-only';
const cleanEnv = { ...process.env };
delete cleanEnv.TAURI_SIGNING_PRIVATE_KEY;
delete cleanEnv.TAURI_SIGNING_PRIVATE_KEY_PASSWORD;
delete cleanEnv.TAURI_SIGNING_PRIVATE_KEY_PATH;
function run(command, args, options = {}) {
  const result = spawnSync(command, args, { cwd: desktop, encoding: 'utf8', env: cleanEnv, ...options });
  if (result.error || result.status !== 0) throw new Error(`${path.basename(command)} failed (${result.status})`);
  return result.stdout;
}
try {
  await writeFile(payload, 'momso local updater payload');
  await writeFile(other, 'different local payload');
  run(process.execPath, [cli, 'signer', 'generate', '--ci', '--password', password, '--write-keys', key]);
  run(process.execPath, [cli, 'signer', 'sign', '--private-key-path', key, '--password', password, payload]);
  run(process.execPath, [cli, 'signer', 'sign', '--private-key-path', key, '--password', password, other]);
  const env = { ...cleanEnv,
    MOMSO_TEST_UPDATER_PUBLIC_KEY: (await readFile(`${key}.pub`, 'utf8')).trim(),
    MOMSO_TEST_UPDATER_SIGNATURE: (await readFile(`${payload}.sig`, 'utf8')).trim(),
    MOMSO_TEST_UPDATER_BAD_SIGNATURE: (await readFile(`${other}.sig`, 'utf8')).trim(),
    CARGO_PROFILE_DEV_DEBUG: '0', CARGO_PROFILE_TEST_DEBUG: '0', CARGO_INCREMENTAL: '0', CARGO_BUILD_JOBS: '1',
  };
  const cargo = process.platform === 'win32' ? 'cargo.exe' : 'cargo';
  const result = spawnSync(cargo, ['test', '--manifest-path', path.join(desktop, 'src-tauri/Cargo.toml'), '-j', '1', 'local_signed_update_flow', '--', '--ignored', '--test-threads=1'], { cwd: desktop, env, stdio: 'inherit' });
  if (result.error || result.status !== 0) throw new Error('Local updater test failed');
} finally {
  try { await rm(dir, { recursive: true, force: true }); }
  catch (error) { process.stderr.write(`Could not remove temporary test key: ${dir}\n`); throw error; }
}
