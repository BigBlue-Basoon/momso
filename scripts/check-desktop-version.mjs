import { execFileSync } from 'node:child_process'
import { existsSync, readFileSync } from 'node:fs'
import { homedir } from 'node:os'
import { dirname, join } from 'node:path'
import { fileURLToPath } from 'node:url'

const repositoryRoot = dirname(dirname(fileURLToPath(import.meta.url)))
const desktopPackage = JSON.parse(
  readFileSync(join(repositoryRoot, 'apps', 'alpha', 'desktop', 'package.json'), 'utf8'),
)
const tauriConfig = JSON.parse(
  readFileSync(join(repositoryRoot, 'apps', 'alpha', 'desktop', 'src-tauri', 'tauri.conf.json'), 'utf8'),
)

const localCargo = join(homedir(), '.cargo', 'bin', process.platform === 'win32' ? 'cargo.exe' : 'cargo')
const cargo = process.env.CARGO || (existsSync(localCargo) ? localCargo : 'cargo')
const cargoMetadata = JSON.parse(
  execFileSync(
    cargo,
    [
      'metadata',
      '--format-version',
      '1',
      '--no-deps',
      '--manifest-path',
      join(repositoryRoot, 'apps', 'alpha', 'desktop', 'src-tauri', 'Cargo.toml'),
    ],
    { cwd: repositoryRoot, encoding: 'utf8' },
  ),
)
const rustPackage = cargoMetadata.packages.find((candidate) => candidate.name === 'momso-desktop')

if (!rustPackage) {
  throw new Error('Cargo metadata does not contain the momso-desktop package.')
}

const versions = {
  'apps/alpha/desktop/package.json': desktopPackage.version,
  'apps/alpha/desktop/src-tauri/Cargo.toml': rustPackage.version,
  'apps/alpha/desktop/src-tauri/tauri.conf.json': tauriConfig.version,
}
const uniqueVersions = new Set(Object.values(versions))

if (uniqueVersions.size !== 1) {
  throw new Error(`Desktop versions are out of sync:\n${JSON.stringify(versions, null, 2)}`)
}

if (tauriConfig.identifier !== 'app.momso.desktop') {
  throw new Error(`Unexpected desktop identifier: ${tauriConfig.identifier}`)
}

console.log(`momso desktop version ${desktopPackage.version} is synchronized.`)
