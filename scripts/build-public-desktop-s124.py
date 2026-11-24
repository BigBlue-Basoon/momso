"""Build and inspect one public commit without changing the public checkout."""
import argparse
import ctypes
import hashlib
import importlib.util
import json
import os
from pathlib import Path
import shutil
import subprocess
import sys
import tarfile
import tempfile
import time

ROOT = Path(__file__).resolve().parent.parent


def run(args, **kwargs):
    subprocess.run([str(a) for a in args], check=True, **kwargs)


def tracked_files(source):
    result = {}
    for path in source.rglob('*'):
        if path.is_symlink():
            raise ValueError('Public archive contains a symlink')
        if path.is_file():
            result[path.relative_to(source)] = hashlib.sha256(path.read_bytes()).digest()
    return result


def require_unchanged(source, original):
    for path, digest in original.items():
        actual = source / path
        if not actual.is_file() or actual.is_symlink() or hashlib.sha256(actual.read_bytes()).digest() != digest:
            raise ValueError('A source archive file changed during preparation or build')
    for folder in ('apps/alpha/desktop/src', 'apps/alpha/desktop/src-tauri/src', 'apps/alpha/desktop/scripts'):
        root = source / folder
        if root.exists():
            for actual in root.rglob('*'):
                if actual.is_symlink() or (actual.is_file() and actual.relative_to(source) not in original):
                    raise ValueError('An uncommitted source file appeared during preparation or build')


def require_commit_tree(repo, commit, source):
    listing = subprocess.check_output(['git', '-C', str(repo), 'ls-tree', '-rz', '--full-tree', commit])
    expected = {}
    for entry in listing.split(b'\0'):
        if not entry:
            continue
        meta, name = entry.split(b'\t', 1)
        mode, kind, oid = meta.split()
        if kind != b'blob' or mode == b'120000':
            raise ValueError('Submodules and symbolic links require explicit review')
        expected[Path(name.decode('utf8'))] = oid.decode('ascii')
    actual = {p.relative_to(source) for p in source.rglob('*') if p.is_file()}
    if actual != set(expected):
        raise ValueError('Archive file list differs from the public commit')
    for path, oid in expected.items():
        data = (source / path).read_bytes()
        blob = b'blob ' + str(len(data)).encode('ascii') + b'\0' + data
        if hashlib.sha1(blob).hexdigest() != oid:
            raise ValueError('Archive bytes differ from the public commit')


def available_ram():
    class MemoryStatus(ctypes.Structure):
        _fields_ = [('length', ctypes.c_ulong), ('load', ctypes.c_ulong), ('total', ctypes.c_ulonglong),
                    ('available', ctypes.c_ulonglong), ('total_page', ctypes.c_ulonglong),
                    ('available_page', ctypes.c_ulonglong), ('total_virtual', ctypes.c_ulonglong),
                    ('available_virtual', ctypes.c_ulonglong), ('extended', ctypes.c_ulonglong)]
    status = MemoryStatus(); status.length = ctypes.sizeof(status)
    if not ctypes.windll.kernel32.GlobalMemoryStatusEx(ctypes.byref(status)):
        raise ctypes.WinError()
    return status.available


def build_with_lock(source, env):
    attempt = 0
    while True:
        attempt += 1
        print('Waiting for the Windows build lock.', flush=True)
        while True:
            if shutil.disk_usage(Path(os.environ['LOCALAPPDATA'])).free < 30 * 1024**3:
                raise RuntimeError('Less than 30 GiB available while waiting')
            if available_ram() >= 4 * 1024**3:
                locked = subprocess.run(['git', '-C', str(ROOT), 'branch', 'hq/build-lock', 'HEAD'], capture_output=True)
                if locked.returncode == 0:
                    if available_ram() >= 4 * 1024**3:
                        break
                    run(['git', '-C', ROOT, 'branch', '-D', 'hq/build-lock'])
                elif subprocess.run(['git', '-C', str(ROOT), 'rev-parse', '--verify', '-q', 'hq/build-lock'], capture_output=True).returncode:
                    raise RuntimeError('Cannot acquire build lock')
            time.sleep(300)
        started = time.monotonic()
        owner = subprocess.check_output(['git', '-C', str(ROOT), 'rev-parse', 'hq/build-lock'], text=True).strip()
        process = None
        timed_out = False
        try:
            print(f'Building the public commit (attempt {attempt}).', flush=True)
            process = subprocess.Popen(['cmd.exe', '/d', '/c', 'npm', 'run', 'desktop:build'], cwd=source, env=env)
            while process.poll() is None:
                if shutil.disk_usage(Path(os.environ['LOCALAPPDATA'])).free < 30 * 1024**3:
                    raise RuntimeError('Less than 30 GiB available during build')
                # Leave time to stop the process tree before the 75-minute lock limit.
                if time.monotonic() - started >= 74 * 60:
                    timed_out = True
                    break
                time.sleep(10)
            if not timed_out and process.returncode:
                raise RuntimeError(f'Public build failed with exit code {process.returncode}; work directory retained')
        finally:
            if process is not None and process.poll() is None:
                subprocess.run(['taskkill', '/PID', str(process.pid), '/T', '/F'], capture_output=True)
                try:
                    process.wait(timeout=30)
                except subprocess.TimeoutExpired:
                    raise RuntimeError('Build process is still running; build lock retained')
            lock = subprocess.run(['git', '-C', str(ROOT), 'rev-parse', '--verify', 'hq/build-lock'], capture_output=True, text=True)
            if lock.returncode == 0 and lock.stdout.strip() == owner:
                run(['git', '-C', ROOT, 'branch', '-D', 'hq/build-lock'])
        if not timed_out:
            return
        print('Build time limit reached; releasing lock before resuming with the same source and target.', flush=True)


def main():
    parser = argparse.ArgumentParser()
    parser.add_argument('--commit', required=True)
    parser.add_argument('--public-repo', type=Path, default=Path.home() / 'Documents/dev/momso-public')
    parser.add_argument('--private-terms-file', type=Path, default=Path(os.environ['LOCALAPPDATA']) / 'momso-s118-private-terms.txt')
    parser.add_argument('--output', type=Path, required=True)
    parser.add_argument('--target', type=Path, default=ROOT / 'target')
    args = parser.parse_args()
    spec = importlib.util.spec_from_file_location('release_s124', ROOT / 'scripts/desktop-release-s118.py')
    release = importlib.util.module_from_spec(spec); spec.loader.exec_module(release)
    release.verify_public_commit(args.public_repo, args.commit)
    terms = args.private_terms_file.resolve(strict=True)
    output = args.output.resolve()
    if output.exists():
        raise ValueError('Output already exists; it will not be replaced')
    if shutil.disk_usage(Path(os.environ['LOCALAPPDATA'])).free < 30 * 1024**3:
        raise RuntimeError('Less than 30 GiB available')
    scratch = Path(tempfile.mkdtemp(prefix='momso-s124-public-'))
    print('Public build work directory:', scratch, flush=True)
    source = scratch / 'source'; source.mkdir()
    archive = scratch / 'source.tar'
    run(['git', '-C', args.public_repo, '-c', 'core.autocrlf=false', 'archive', '--format=tar', '-o', archive, args.commit])
    with tarfile.open(archive) as tar:
        tar.extractall(source, filter='data')
    require_commit_tree(args.public_repo, args.commit, source)
    original = tracked_files(source)
    config = json.loads((source / 'apps/alpha/desktop/src-tauri/tauri.conf.json').read_text(encoding='utf8'))
    if config['version'] != '0.1.1' or config['plugins']['updater']['endpoints'][0] != release.REPOSITORY + '/releases/latest/download/latest.json':
        raise ValueError('Version or update endpoint differs from the approved release')
    clean_env = os.environ.copy()
    for key in ('TAURI_SIGNING_PRIVATE_KEY', 'TAURI_SIGNING_PRIVATE_KEY_PATH', 'TAURI_SIGNING_PRIVATE_KEY_PASSWORD'):
        clean_env.pop(key, None)
    print('Installing the public commit dependencies.', flush=True)
    run(['cmd.exe', '/d', '/c', 'npm', 'ci', '--no-audit', '--no-fund'], cwd=source, env=clean_env)
    require_unchanged(source, original)
    target = args.target.resolve()
    env = clean_env.copy()
    env.update(CARGO_TARGET_DIR=str(target), CARGO_BUILD_JOBS='1', CARGO_INCREMENTAL='0',
               CARGO_PROFILE_DEV_DEBUG='0', CARGO_PROFILE_TEST_DEBUG='0', MOMSO_RELEASE_PRIVATE_TERMS_FILE=str(terms))
    build_with_lock(source, env)
    require_unchanged(source, original)
    installer = target / 'release/bundle/nsis/momso_0.1.1_x64-setup.exe'
    if not installer.is_file():
        raise RuntimeError('Expected unsigned NSIS is missing')
    output.mkdir(parents=True)
    copied = output / installer.name
    shutil.copy2(installer, copied)
    print('Inspecting and starting the actual product.', flush=True)
    run([sys.executable, ROOT / 'scripts/verify-product-current-user-s118.py', '--installer', copied,
         '--private-terms-file', terms, '--source-root', source, '--built-exe', target / 'release/momso-desktop.exe'])
    run([sys.executable, ROOT / 'scripts/desktop-release-s118.py', 'record-startup', '--installer', copied,
         '--private-terms-file', terms, '--startup-evidence', output / 'startup-evidence.json'])
    require_unchanged(source, original)
    release.verify_public_commit(args.public_repo, args.commit)
    signer = output / 'signer'
    (signer / 'node_modules/@tauri-apps').mkdir(parents=True)
    shutil.copy2(source / 'apps/alpha/desktop/src-tauri/generated/node/node.exe', signer / 'node.exe')
    for package in ('cli', 'cli-win32-x64-msvc'):
        shutil.copytree(source / 'node_modules/@tauri-apps' / package, signer / 'node_modules/@tauri-apps' / package)
    release.check_tree(signer, terms)
    receipt_file = output / 'momso-release-preparation.json'
    receipt = json.loads(receipt_file.read_text(encoding='utf8'))
    receipt['sourceCommitSha'] = args.commit
    receipt['sourceTreeSha'] = subprocess.check_output(['git', '-C', str(args.public_repo), 'rev-parse', args.commit + '^{tree}'], text=True).strip()
    receipt['signerTreeSha256'] = release.signer_tree_hash(signer)
    receipt_file.write_text(json.dumps(receipt, indent=2) + '\n', encoding='utf8')
    print('Prepared public installer:', copied, flush=True)
    print('Build target retained for owner cleanup:', target, flush=True)
    if scratch.resolve().parent != Path(tempfile.gettempdir()).resolve() or not scratch.name.startswith('momso-s124-public-'):
        raise RuntimeError('Unexpected work path; retained')
    try:
        shutil.rmtree(scratch)
    except OSError:
        print('Build work directory retained:', scratch, flush=True)


if __name__ == '__main__':
    main()
