import { execFileSync } from 'node:child_process';
import { writeFileSync } from 'node:fs';
import path from 'node:path';
import { fileURLToPath } from 'node:url';

const desktop = path.resolve(path.dirname(fileURLToPath(import.meta.url)), '..');
const root = path.resolve(desktop, '../../..');
const cargo = JSON.parse(execFileSync('cargo', ['metadata', '--locked', '--offline', '--format-version', '1'], {
  cwd: path.join(desktop, 'src-tauri'), encoding: 'utf8', maxBuffer: 32 * 1024 * 1024,
}));
const npm = JSON.parse(execFileSync(process.env.ComSpec || 'cmd.exe', [
  '/d', '/s', '/c', 'npm query --package-lock-only --workspace @momso/alpha-desktop * --json',
], { cwd: root, encoding: 'utf8', maxBuffer: 32 * 1024 * 1024 }));
const rust = cargo.packages.filter((p) => p.source).map((p) => ({
  name: p.name, version: p.version, license: p.license ?? null,
  licenseFile: p.license_file ? path.basename(p.license_file) : null,
  source: p.repository ?? null,
})).sort((a, b) => a.name.localeCompare(b.name) || a.version.localeCompare(b.version));
const packages = npm.filter((p) => p.version && p.location?.includes('node_modules/')).map((p) => ({
  name: p.name, version: p.version, license: p.license ?? null,
})).sort((a, b) => a.name.localeCompare(b.name) || a.version.localeCompare(b.version));
const report = {
  sources: {
    rust: 'cargo metadata --locked --offline --format-version 1; all resolved targets including build dependencies',
    npm: 'npm query --package-lock-only --workspace @momso/alpha-desktop * --json; lockfile inventory may include other workspace dependencies',
  },
  rust,
  npm: packages,
};
writeFileSync(path.join(desktop, 'third-party', 'dependency-licenses.json'), JSON.stringify(report, null, 2) + '\n', 'utf8');
process.stdout.write(`Rust: ${rust.length}; npm: ${packages.length}\n`);
