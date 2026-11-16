import { execFileSync } from 'node:child_process'
import { createHash } from 'node:crypto'
import { readFileSync, readdirSync, statSync, writeFileSync } from 'node:fs'
import { dirname, join, resolve } from 'node:path'
import { fileURLToPath } from 'node:url'

const scriptPath = fileURLToPath(import.meta.url)
const repositoryRoot = dirname(dirname(scriptPath))
const defaultInstallerDirectory = join(
  repositoryRoot,
  'apps',
  'alpha',
  'desktop',
  'src-tauri',
  'target',
  'release',
  'bundle',
  'nsis',
)

function readJson(path) {
  return JSON.parse(readFileSync(path, 'utf8'))
}

function runGit(args) {
  try {
    return execFileSync('git', args, {
      cwd: repositoryRoot,
      encoding: 'utf8',
      stdio: ['ignore', 'pipe', 'ignore'],
    }).trim()
  } catch {
    return null
  }
}

function githubRepositoryFromRemote(remote) {
  if (!remote) {
    return null
  }

  const match = remote.match(/github\.com[/:]([^/]+\/[^/]+?)(?:\.git)?$/i)
  return match?.[1] ?? null
}

function sha256(path) {
  return createHash('sha256').update(readFileSync(path)).digest('hex')
}

function readGithubEvent(environment) {
  if (!environment.GITHUB_EVENT_PATH) {
    return null
  }

  return readJson(environment.GITHUB_EVENT_PATH)
}

function requireExactlyOneInstaller(installerDirectory) {
  const installers = readdirSync(installerDirectory, { withFileTypes: true })
    .filter((entry) => entry.isFile() && entry.name.toLowerCase().endsWith('.exe'))
    .map((entry) => entry.name)

  if (installers.length !== 1) {
    throw new Error(
      `Expected exactly one NSIS installer in ${installerDirectory}, found ${installers.length}.`,
    )
  }

  return installers[0]
}

function normalizeArchitecture(environment, architecture) {
  const value = environment.RUNNER_ARCH || architecture
  const normalized = value.toLowerCase()

  if (normalized === 'x64' || normalized === 'x86_64') {
    return 'x64'
  }

  if (normalized === 'arm64' || normalized === 'aarch64') {
    return 'arm64'
  }

  return normalized
}

export function createDesktopArtifactMetadata({
  installerDirectory = defaultInstallerDirectory,
  environment = process.env,
  now = () => new Date(),
  architecture = process.arch,
  git = runGit,
} = {}) {
  const desktopPackage = readJson(join(repositoryRoot, 'apps', 'alpha', 'desktop', 'package.json'))
  const tauriConfig = readJson(
    join(repositoryRoot, 'apps', 'alpha', 'desktop', 'src-tauri', 'tauri.conf.json'),
  )

  if (desktopPackage.version !== tauriConfig.version) {
    throw new Error('Desktop package and Tauri versions must match before packaging metadata.')
  }

  if (!tauriConfig.bundle?.targets?.includes('nsis')) {
    throw new Error('Tauri must include the NSIS bundle target.')
  }

  if (tauriConfig.bundle?.windows?.nsis?.installMode !== 'currentUser') {
    throw new Error('The manifest expects an NSIS current-user installer.')
  }

  const resolvedInstallerDirectory = resolve(installerDirectory)
  const installerName = requireExactlyOneInstaller(resolvedInstallerDirectory)
  const installerPath = join(resolvedInstallerDirectory, installerName)
  const installerHash = sha256(installerPath)
  const checksumName = `${installerName}.sha256`
  const checksumPath = join(resolvedInstallerDirectory, checksumName)
  const manifestName = 'momso-build-manifest.json'
  const manifestPath = join(resolvedInstallerDirectory, manifestName)
  const repository =
    environment.GITHUB_REPOSITORY || githubRepositoryFromRemote(git(['remote', 'get-url', 'origin']))
  const commit = environment.GITHUB_SHA || git(['rev-parse', 'HEAD'])
  const ref = environment.GITHUB_REF || git(['branch', '--show-current'])
  const runId = environment.GITHUB_RUN_ID || null
  const serverUrl = environment.GITHUB_SERVER_URL || 'https://github.com'
  const isGitHubActions = environment.GITHUB_ACTIONS === 'true'
  const githubEvent = readGithubEvent(environment)
  const dirtyOutput = isGitHubActions ? '' : git(['status', '--porcelain'])

  if (!commit) {
    throw new Error('Could not determine the source commit for the installer.')
  }

  const manifest = {
    schemaVersion: 1,
    product: {
      name: tauriConfig.productName,
      channel: 'alpha',
      version: desktopPackage.version,
      identifier: tauriConfig.identifier,
    },
    source: {
      repository,
      checkoutCommit: commit,
      headCommit: githubEvent?.pull_request?.head?.sha || commit,
      ref: ref || null,
      headRef: environment.GITHUB_HEAD_REF || null,
      baseRef: environment.GITHUB_BASE_REF || null,
      pullRequestNumber: githubEvent?.pull_request?.number || null,
      dirty: isGitHubActions ? false : Boolean(dirtyOutput),
    },
    build: {
      provider: isGitHubActions ? 'github-actions' : 'local',
      workflow: environment.GITHUB_WORKFLOW || null,
      job: environment.GITHUB_JOB || null,
      event: environment.GITHUB_EVENT_NAME || null,
      runId,
      runNumber: environment.GITHUB_RUN_NUMBER || null,
      runAttempt: environment.GITHUB_RUN_ATTEMPT || null,
      runUrl: repository && runId ? `${serverUrl}/${repository}/actions/runs/${runId}` : null,
      generatedAt: now().toISOString(),
    },
    distribution: {
      platform: 'windows',
      architecture: normalizeArchitecture(environment, architecture),
      installerType: 'nsis',
      installScope: 'currentUser',
      signatureStatus: 'unsigned',
      updaterEnabled: false,
      intendedUse: 'testing',
    },
    artifact: {
      fileName: installerName,
      sizeBytes: statSync(installerPath).size,
      sha256: installerHash,
      checksumFile: checksumName,
      retentionDays: 14,
    },
  }

  writeFileSync(checksumPath, `${installerHash}  ${installerName}\n`, 'ascii')
  writeFileSync(manifestPath, `${JSON.stringify(manifest, null, 2)}\n`, 'utf8')

  return { checksumPath, installerPath, manifest, manifestPath }
}

if (process.argv[1] && resolve(process.argv[1]) === resolve(scriptPath)) {
  const installerDirectory = process.argv[2] || defaultInstallerDirectory
  const result = createDesktopArtifactMetadata({ installerDirectory })
  console.log(
    `Wrote unsigned installer provenance for ${result.manifest.artifact.fileName} (${result.manifest.artifact.sha256}).`,
  )
}
