"""Exercise a real, separately named Tauri NSIS update on an isolated desktop.

Windows only. Run under the shared Windows build lock; uses a temporary signer key.
"""
import hashlib
import io
import ctypes
from ctypes import wintypes
import http.server
import importlib.util
import json
import os
from pathlib import Path
import shutil
import subprocess
import sys
import tempfile
import threading
import time
import uuid
import urllib.request
import zipfile

ROOT = Path(__file__).resolve().parent.parent
DESKTOP = ROOT / 'apps/alpha/desktop'
TAURI = ROOT / 'node_modules/@tauri-apps/cli/tauri.js'

NODE_VERSION = '22.23.5'
NODE_SHA256 = '4a191082e9706866830379d577d2cc8a50c776fcd9436a8d061b063ed0d9a0e3'


def stage_node(destination):
    """Stage the same checksum-pinned Node archive as the release builder."""
    url = f'https://nodejs.org/dist/v{NODE_VERSION}/node-v{NODE_VERSION}-win-x64.zip'
    with urllib.request.urlopen(url, timeout=120) as response:
        data = response.read(100_000_001)
    if len(data) > 100_000_000 or hashlib.sha256(data).hexdigest() != NODE_SHA256:
        raise RuntimeError('Node fixture archive checksum mismatch')
    prefix = f'node-v{NODE_VERSION}-win-x64/'
    with zipfile.ZipFile(io.BytesIO(data)) as archive:
        for info in archive.infolist():
            if not info.filename.startswith(prefix):
                raise RuntimeError('unexpected Node archive entry')
            relative = info.filename[len(prefix):].rstrip('/')
            if not relative:
                continue
            parts = relative.split('/')
            if any(part in ('', '.', '..') or ':' in part or chr(92) in part for part in parts):
                raise RuntimeError('unsafe Node archive entry')
            if parts[:2] == ['node_modules', 'corepack'] or (parts[:2] == ['node_modules', 'npm'] and len(parts) > 2 and parts[2] in ('docs', 'html', 'man')) or parts[-1].lower().split('.')[0] in ('readme', 'changelog', 'history', 'contributing'):
                continue
            output = destination.joinpath(*parts)
            if len(str(output)) > 259:
                raise RuntimeError('fixture resource path exceeds NSIS limit')
            if info.is_dir():
                output.mkdir(parents=True, exist_ok=True)
            else:
                output.parent.mkdir(parents=True, exist_ok=True)
                with archive.open(info) as source, output.open('xb') as target:
                    shutil.copyfileobj(source, target)
    if not (destination / 'node.exe').is_file() or not (destination / 'node_modules/npm/bin/npm-cli.js').is_file():
        raise RuntimeError('Node fixture runtime incomplete')


def stage_crt(root, node_stage, env):
    """Use the release builder's discovery and Microsoft signature check."""
    js = "import {locateMsvcCrt} from './scripts/build-desktop.mjs'; console.log(JSON.stringify(await locateMsvcCrt()))"
    result = subprocess.run(['node', '--input-type=module', '-e', js], cwd=DESKTOP, env=env, check=True, capture_output=True, text=True)
    crt = json.loads(result.stdout)
    crt_source = Path(crt['folder'])
    check_env = dict(env, MOMSO_CRT_SOURCE=str(crt_source), MOMSO_CRT_NAMES=json.dumps(crt['names']))
    check = "$ErrorActionPreference='Stop'; Import-Module (Join-Path $env:SystemRoot 'System32\\WindowsPowerShell\\v1.0\\Modules\\Microsoft.PowerShell.Security\\Microsoft.PowerShell.Security.psd1'); foreach ($name in (ConvertFrom-Json $env:MOMSO_CRT_NAMES)) { $sig = Get-AuthenticodeSignature -LiteralPath (Join-Path $env:MOMSO_CRT_SOURCE $name); if ($sig.Status -ne 'Valid' -or $sig.SignerCertificate.Subject -notmatch 'O=Microsoft Corporation') { throw 'Invalid Microsoft runtime signature' } }"
    subprocess.run(['powershell.exe', '-NoProfile', '-NonInteractive', '-Command', check], env=check_env, check=True, creationflags=0x08000000)
    crt_stage = root / 'crt'
    crt_stage.mkdir()
    resources = {}
    for name in crt['names']:
        shutil.copy2(crt_source / name, crt_stage / name)
        shutil.copy2(crt_source / name, node_stage / name)
        resources[(crt_stage / name).as_posix()] = name
    return resources


def main():
    spec = importlib.util.spec_from_file_location('isolated_window', ROOT / 'scripts/test-s5b-window.py')
    fixture = importlib.util.module_from_spec(spec)
    spec.loader.exec_module(fixture)
    with tempfile.TemporaryDirectory(prefix='momso-s69-updater-', ignore_cleanup_errors=True) as temp:
        root = Path(temp)
        identity = uuid.uuid4().hex[:12]
        name = 'momso-s69-' + identity
        def cleanup_artifacts():
            debug = ROOT / 'target/debug'
            owned = [debug / (name + '.exe')]
            for version in ('0.1.1', '0.1.2'):
                bundle = debug / 'bundle/nsis' / f'{name}_{version}_x64-setup.exe'
                owned.extend((bundle, Path(str(bundle) + '.sig')))
            for path in owned:
                try:
                    path.unlink(missing_ok=True)
                except OSError:
                    print('fixture_artifact_cleanup_needed=' + str(path), flush=True)
        marker = root / 'result.txt'
        signing_key = root / 'fixture.key'
        env = os.environ.copy()
        for key in list(env):
            if key.startswith('TAURI_SIGNING_') or key in ('OPENAI_API_KEY', 'CODEX_API_KEY', 'ANTHROPIC_API_KEY', 'GEMINI_API_KEY', 'GOOGLE_API_KEY', 'GH_TOKEN', 'GITHUB_TOKEN', 'CODEX_HOME', 'CLAUDE_CONFIG_DIR', 'GOOGLE_APPLICATION_CREDENTIALS', 'GH_CONFIG_DIR', 'WEBVIEW2_ADDITIONAL_BROWSER_ARGUMENTS'):
                env.pop(key, None)
        env.update(CARGO_TARGET_DIR=str(ROOT / 'target'), CARGO_BUILD_JOBS='1', CARGO_INCREMENTAL='0', CARGO_PROFILE_DEV_DEBUG='0', CARGO_PROFILE_TEST_DEBUG='0', MOMSO_TEST_LOCAL_UPDATER='1', MOMSO_TEST_LOCAL_UPDATER_FILE=str(marker), MOMSO_TEST_FAKE_CREDENTIAL_STORE='1', MOMSO_S69_TEST_ROOT=str(root / 'runtime'))
        build_env = env.copy()
        env['USERPROFILE'] = str(root / 'user')
        env['APPDATA'] = str(root / 'user/AppData/Roaming')
        env['LOCALAPPDATA'] = str(root / 'user/AppData/Local')
        for path in ('APPDATA', 'LOCALAPPDATA'):
            Path(env[path]).mkdir(parents=True, exist_ok=True)
        signer_env = build_env.copy()
        subprocess.run(['node', str(TAURI), 'signer', 'generate', '--ci', '--password', 'fixture-only', '--write-keys', str(signing_key)], cwd=DESKTOP, env=signer_env, check=True)
        pubkey = signing_key.with_suffix('.key.pub').read_text(encoding='utf-8').strip()
        server_state = {}
        class Handler(http.server.BaseHTTPRequestHandler):
            def log_message(self, *_): pass
            def do_GET(self):
                data = server_state['json'] if self.path == '/latest.json' else server_state['installer']
                self.send_response(200)
                self.send_header('Content-Length', str(len(data)))
                self.end_headers()
                self.wfile.write(data)
        httpd = http.server.ThreadingHTTPServer(('127.0.0.1', 0), Handler)
        thread = threading.Thread(target=httpd.serve_forever, daemon=True)
        thread.start()
        port = httpd.server_port
        # Use short staging paths and the same pinned runtime and signed CRT as release.
        node_stage = root / 'node'
        notices_stage = root / 'third-party'
        stage_node(node_stage)
        shutil.copytree(DESKTOP / 'third-party', notices_stage)
        resources = {'generated/node/': None, '../third-party/': None,
                     node_stage.as_posix() + '/': 'node/', notices_stage.as_posix() + '/': 'third-party/'}
        resources.update(stage_crt(root, node_stage, build_env))
        config = {'productName': name, 'identifier': 'app.momso.fixture.' + identity,
                  'mainBinaryName': name, 'app': {'windows': [{'label': 'main', 'visible': False, 'focus': False}]},
                  'build': {'beforeBuildCommand': ''},
                  'bundle': {'createUpdaterArtifacts': True, 'resources': resources},
                  'plugins': {'updater': {'pubkey': pubkey, 'endpoints': [f'http://127.0.0.1:{port}/latest.json'], 'dangerousInsecureTransportProtocol': True, 'windows': {'installMode': 'quiet'}}}}
        build_env['TAURI_SIGNING_PRIVATE_KEY'] = str(signing_key)
        build_env['TAURI_SIGNING_PRIVATE_KEY_PASSWORD'] = 'fixture-only'
        artifacts = {}
        try:
            for version in ('0.1.1', '0.1.2'):
                config['version'] = version
                config_path = root / ('config-' + version + '.json')
                config_path.write_text(json.dumps(config), encoding='utf-8')
                subprocess.run(['node', str(TAURI), 'build', '--debug', '--features', 'g61-test-probe', '--bundles', 'nsis', '--config', str(config_path), '--', '-j', '1'], cwd=DESKTOP, env=build_env, check=True)
                artifact = ROOT / 'target/debug/bundle/nsis' / f'{name}_{version}_x64-setup.exe'
                if not artifact.is_file():
                    raise RuntimeError('fixture installer missing: ' + str(artifact))
                artifacts[version] = (artifact.read_bytes(), Path(str(artifact) + '.sig').read_text(encoding='utf-8').strip())
            # A signed non-installer tests failure after verification but before launch.
            invalid_installer = root / 'invalid-installer.exe'
            invalid_installer.write_bytes(b"not-a-pe-" + os.urandom(128))
            subprocess.run(['node', str(TAURI), 'signer', 'sign', '--private-key-path', str(signing_key), '--password', 'fixture-only', str(invalid_installer)], cwd=DESKTOP, env=signer_env, check=True, stdout=subprocess.DEVNULL)
            interrupted = root / 'interrupted-installer.exe'
            nsis = root / 'interrupted-installer.nsi'
            nsis.write_text("\n".join([
                'Name "momso S69 interruption fixture"',
                'OutFile "' + str(interrupted) + '"',
                'SilentInstall silent',
                'RequestExecutionLevel user',
                'Section',
                'Sleep 5000',
                'ReadEnvStr $0 "MOMSO_S69_TEST_EXE"',
                'FileOpen $1 "$0" w',
                'IfErrors 0 +2',
                'Abort',
                'FileWrite $1 "fixture-partial-write"',
                'FileClose $1',
                'ReadEnvStr $0 "MOMSO_S69_TEST_NEWFILE"',
                'FileOpen $1 "$0" w',
                'FileWrite $1 "new-install-file"',
                'FileClose $1',
                'ReadEnvStr $0 "MOMSO_S69_TEST_INSTALLER_PID"',
                "System::Call 'kernel32::GetCurrentProcessId()i.r2'",
                'FileOpen $1 "$0" w',
                'FileWrite $1 "$2"',
                'FileClose $1',
                'Sleep 300000',
                'SectionEnd',
            ]), encoding='utf-8')
            nsis_compiler = Path(os.environ['LOCALAPPDATA']) / 'tauri/NSIS/makensis.exe'
            subprocess.run([str(nsis_compiler), str(nsis)], cwd=root, env=signer_env, check=True, stdout=subprocess.DEVNULL, creationflags=0x08000000)
            subprocess.run(['node', str(TAURI), 'signer', 'sign', '--private-key-path', str(signing_key), '--password', 'fixture-only', str(interrupted)], cwd=DESKTOP, env=signer_env, check=True, stdout=subprocess.DEVNULL)
            # The key is no longer needed once all fixture files are signed.
            signing_key.unlink()
            server_state['installer'] = artifacts['0.1.2'][0]
            server_state['json'] = json.dumps({'version': '0.1.2', 'platforms': {'windows-x86_64': {'url': f'http://127.0.0.1:{port}/installer.exe', 'signature': artifacts['0.1.2'][1]}}}).encode()
            first = root / 'initial.exe'
            first.write_bytes(artifacts['0.1.1'][0])
            install = root / 'installed'
            # The unique fixture product and explicit directory keep the real app untouched.
            subprocess.run([str(first), '/S', '/D=' + str(install)], env=env, check=True, timeout=120, creationflags=0x08000000)
            executable = install / (name + '.exe')
            if not executable.is_file(): raise RuntimeError('initial fixture installation failed')
            for relative in (Path('node.exe'), Path('node_modules/npm/bin/npm-cli.js')):
                expected = node_stage / relative
                actual = install / 'node' / relative
                if not actual.is_file() or hashlib.sha256(actual.read_bytes()).digest() != hashlib.sha256(expected.read_bytes()).digest():
                    raise RuntimeError('installed Node runtime differs from verified release archive')
            for source, destination in resources.items():
                if source.endswith('.dll'):
                    actual = install / destination
                    if not actual.is_file() or hashlib.sha256(actual.read_bytes()).digest() != hashlib.sha256(Path(source).read_bytes()).digest():
                        raise RuntimeError('installed CRT differs from Microsoft-signed runtime')
            print('PASS installed pinned Node runtime and Microsoft-signed CRT match', flush=True)
            desktop_name = 'momso-s69-' + identity
            desktop = fixture.user.CreateDesktopW(desktop_name, None, None, 0, 0x10000000, None)
            if not desktop: raise ctypes.WinError(ctypes.get_last_error())
            process = fixture.ProcessInfo()
            try:
                startup = fixture.StartupInfo()
                startup.cb, startup.desktop = ctypes.sizeof(startup), desktop_name
                block = ctypes.create_unicode_buffer('\0'.join(f'{k}={v}' for k,v in sorted(env.items(), key=lambda x:x[0].upper())) + '\0\0')
                command = ctypes.create_unicode_buffer(subprocess.list2cmdline([str(executable)]))
                if not fixture.kernel.CreateProcessW(str(executable), command, None, None, False, 0x08000400, block, None, ctypes.byref(startup), ctypes.byref(process)):
                    raise ctypes.WinError(ctypes.get_last_error())
                fixture.kernel.CloseHandle(process.thread)
                deadline = time.monotonic() + 240
                while time.monotonic() < deadline:
                    text = marker.read_text(encoding='utf-8') if marker.exists() else ''
                    if text == 'restarted:0.1.2':
                        print('PASS real isolated Tauri update installed and restarted as 0.1.2', flush=True)
                        break
                    if text.endswith('failed') or text.startswith('install_failed:'):
                        raise RuntimeError(text)
                    time.sleep(1)
                else: raise RuntimeError('fixture update or restart timed out; marker=' + (marker.read_text() if marker.exists() else 'missing'))
                recovery = root / 'runtime/data/backups/update-recovery'
                deadline = time.monotonic() + 90
                while recovery.exists() and any(recovery.iterdir()) and time.monotonic() < deadline:
                    time.sleep(1)
                if recovery.exists() and any(recovery.iterdir()): raise RuntimeError('healthy update backup was not cleaned')
                print('PASS healthy update removes its recovery snapshot', flush=True)
            finally:
                if process.process:
                    if fixture.kernel.WaitForSingleObject(process.process, 0) != 0: fixture.kernel.TerminateProcess(process.process, 1)
                    fixture.kernel.CloseHandle(process.process)
                fixture.user.CloseDesktop(desktop)
        finally:
            try:
                signing_key.unlink(missing_ok=True)
            except OSError:
                print('fixture_key_cleanup_needed=' + str(signing_key), flush=True)
            httpd.shutdown(); httpd.server_close(); thread.join(timeout=5)
            # NSIS writes a unique per-user uninstall record. The test runner prints
            # its product name so it can be checked if uninstall cannot complete.
            uninstallers = list((root / 'installed').glob('*uninstall*.exe')) if (root / 'installed').exists() else []
            uninstaller = uninstallers[0] if len(uninstallers) == 1 else root / 'uninstall-not-found'
            if (root / 'installed').exists() and not uninstaller.is_file():
                print('fixture_uninstall_missing=' + name, flush=True)
            if uninstaller.is_file():
                result = subprocess.run([str(uninstaller), '/S'], env=env, timeout=120, creationflags=0x08000000)
                if result.returncode: print('fixture_uninstall_failed=' + name, flush=True)
            time.sleep(5)
            if sys.exc_info()[0] is not None:
                cleanup_artifacts()
        try:
            subprocess.run([sys.executable, str(ROOT / 'scripts/test-desktop-updater-s69-retry.py'), str(root / 'config-0.1.1.json'), str(ROOT / 'target/debug/bundle/nsis' / f'{name}_0.1.1_x64-setup.exe'), str(ROOT / 'target/debug/bundle/nsis' / f'{name}_0.1.2_x64-setup.exe')], env=env, check=True)
            subprocess.run([sys.executable, str(ROOT / 'scripts/test-desktop-updater-s69-retry.py'), str(root / 'config-0.1.1.json'), str(ROOT / 'target/debug/bundle/nsis' / f'{name}_0.1.1_x64-setup.exe'), str(invalid_installer), 'installer'], env=env, check=True)
            if os.environ.get('MOMSO_S69_TEST_INTERRUPT') == '1':
                subprocess.run([sys.executable, str(ROOT / 'scripts/test-desktop-updater-s69-retry.py'), str(root / 'config-0.1.1.json'), str(ROOT / 'target/debug/bundle/nsis' / f'{name}_0.1.1_x64-setup.exe'), str(interrupted), 'interrupt'], env=env, check=True)
                subprocess.run([sys.executable, str(ROOT / 'scripts/test-desktop-updater-s69-retry.py'), str(root / 'config-0.1.1.json'), str(ROOT / 'target/debug/bundle/nsis' / f'{name}_0.1.1_x64-setup.exe'), str(interrupted), 'power'], env=env, check=True)
        finally:
            cleanup_artifacts()
    if root.exists(): print('fixture_cleanup_needed=' + str(root), flush=True)


if __name__ == '__main__': main()
