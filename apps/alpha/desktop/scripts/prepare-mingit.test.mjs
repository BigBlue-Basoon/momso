import assert from 'node:assert/strict';
import { createHash } from 'node:crypto';
import { mkdtemp, mkdir, rm, writeFile } from 'node:fs/promises';
import os from 'node:os';
import path from 'node:path';
import { test } from 'node:test';
import { strToU8, zipSync } from 'fflate';
import { decodeVerifiedArchive, verifyExisting } from './prepare-mingit.mjs';

function fixture(entries) {
  const bytes = zipSync(entries);
  return [bytes, createHash('sha256').update(bytes).digest('hex')];
}
test('verifies digest before unpacking and requires the executable', () => {
  const [bytes, hash] = fixture({ 'cmd/git.exe': strToU8('git') });
  assert.equal(decodeVerifiedArchive(bytes, hash).size, 1);
  assert.throws(() => decodeVerifiedArchive(bytes, '0'.repeat(64)), /SHA-256/);
  const [missing, missingHash] = fixture({ 'other': strToU8('data') });
  assert.throws(() => decodeVerifiedArchive(missing, missingHash), /executable/);
});
test('rejects path traversal even for a matching digest', () => {
  const [bytes, hash] = fixture({ '../escape': strToU8('x'), 'cmd/git.exe': strToU8('git') });
  assert.throws(() => decodeVerifiedArchive(bytes, hash), /Unsafe/);
});
test('detects modified or additional generated files', async () => {
  const dir = await mkdtemp(path.join(os.tmpdir(), 'momso-mingit-test-'));
  try {
    const files = new Map([['cmd/git.exe', strToU8('git')]]);
    await mkdir(path.join(dir, 'cmd'));
    await writeFile(path.join(dir, 'cmd/git.exe'), 'git');
    await verifyExisting(dir, files);
    await writeFile(path.join(dir, 'cmd/git.exe'), 'changed');
    await assert.rejects(verifyExisting(dir, files), /differs/);
    await writeFile(path.join(dir, 'extra'), 'extra');
    await assert.rejects(verifyExisting(dir, files), /differs/);
  } finally { await rm(dir, { recursive: true, force: true }); }
});
