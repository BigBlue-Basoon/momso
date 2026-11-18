import assert from 'node:assert/strict'
import { createHash } from 'node:crypto'
import { mkdtempSync, readFileSync, rmSync, writeFileSync } from 'node:fs'
import { tmpdir } from 'node:os'
import { join } from 'node:path'
import { fileURLToPath } from 'node:url'
import test from 'node:test'

import { createDesktopArtifactMetadata } from './create-desktop-artifact-metadata.mjs'

test('writes a checksum and provenance manifest for one unsigned NSIS installer', () => {
  const directory = mkdtempSync(join(tmpdir(), 'momso-artifact-'))

  try {
    const installerName = 'momso_0.1.0_x64-setup.exe'
    const installerBody = Buffer.from('test-installer')
    const eventPath = join(directory, 'event.json')
    writeFileSync(join(directory, installerName), installerBody)
    writeFileSync(
      eventPath,
      JSON.stringify({
        pull_request: {
          number: 44,
          head: { sha: 'fedcba9876543210fedcba9876543210fedcba98' },
        },
      }),
    )

    const result = createDesktopArtifactMetadata({
      installerDirectory: directory,
      environment: {
        GITHUB_ACTIONS: 'true',
        GITHUB_REPOSITORY: 'example/project',
        GITHUB_SHA: '0123456789abcdef0123456789abcdef01234567',
        GITHUB_REF: 'refs/pull/44/merge',
        GITHUB_HEAD_REF: 'example/change',
        GITHUB_BASE_REF: 'main',
        GITHUB_EVENT_PATH: eventPath,
        GITHUB_RUN_ID: '1001',
        GITHUB_RUN_NUMBER: '44',
        GITHUB_RUN_ATTEMPT: '1',
        GITHUB_WORKFLOW: 'Desktop alpha',
        GITHUB_JOB: 'windows-build',
        GITHUB_EVENT_NAME: 'pull_request',
        GITHUB_SERVER_URL: 'https://github.com',
        RUNNER_ARCH: 'X64',
      },
      now: () => new Date('2026-07-12T00:00:00.000Z'),
      git: () => {
        throw new Error('Git fallback should not be used in GitHub Actions metadata.')
      },
    })

    const expectedHash = createHash('sha256').update(installerBody).digest('hex')
    const checksum = readFileSync(result.checksumPath, 'ascii')
    const manifest = JSON.parse(readFileSync(result.manifestPath, 'utf8'))

    assert.equal(checksum, `${expectedHash}  ${installerName}\n`)
    assert.equal(manifest.schemaVersion, 1)
    const desktopPackage = JSON.parse(readFileSync(fileURLToPath(new URL('../apps/alpha/desktop/package.json', import.meta.url)), 'utf8'))
    const tauriConfig = JSON.parse(readFileSync(fileURLToPath(new URL('../apps/alpha/desktop/src-tauri/tauri.conf.json', import.meta.url)), 'utf8'))
    assert.equal(desktopPackage.version, tauriConfig.version)
    assert.equal(manifest.product.version, desktopPackage.version)
    assert.equal(
      manifest.source.checkoutCommit,
      '0123456789abcdef0123456789abcdef01234567',
    )
    assert.equal(manifest.source.headCommit, 'fedcba9876543210fedcba9876543210fedcba98')
    assert.equal(manifest.source.pullRequestNumber, 44)
    assert.equal(manifest.source.dirty, false)
    assert.equal(manifest.build.runUrl, 'https://github.com/example/project/actions/runs/1001')
    assert.equal(manifest.distribution.signatureStatus, 'unsigned')
    assert.equal(manifest.distribution.updaterEnabled, false)
    assert.equal(manifest.artifact.sha256, expectedHash)
    assert.equal(manifest.artifact.sizeBytes, installerBody.length)
  } finally {
    rmSync(directory, { recursive: true, force: true })
  }
})

test('rejects a directory with ambiguous installers', () => {
  const directory = mkdtempSync(join(tmpdir(), 'momso-artifact-'))

  try {
    writeFileSync(join(directory, 'first.exe'), 'first')
    writeFileSync(join(directory, 'second.exe'), 'second')

    assert.throws(
      () => createDesktopArtifactMetadata({ installerDirectory: directory }),
      /Expected exactly one NSIS installer/,
    )
  } finally {
    rmSync(directory, { recursive: true, force: true })
  }
})
