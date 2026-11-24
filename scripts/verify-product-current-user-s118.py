"""Inspect and launch the release installer under an explicitly authorized existing user."""
import ctypes
import argparse
import hashlib
import importlib.util
import json
import os
from pathlib import Path
import socket
import subprocess
import tempfile
import time
import urllib.request
import winreg

import websocket

ROOT = Path(__file__).resolve().parent.parent


def module(name, relative):
    spec = importlib.util.spec_from_file_location(name, ROOT / relative)
    value = importlib.util.module_from_spec(spec)
    spec.loader.exec_module(value)
    return value


def snapshot(paths):
    result = {}
    for root in paths:
        if root.exists() and not root.is_symlink():
            for path in root.rglob('*'):
                if path.is_file() and not path.is_symlink():
                    try:
                        stat = path.stat()
                        result[str(path)] = [stat.st_mtime_ns, stat.st_size]
                    except OSError:
                        pass
    return result


def first_screen(port):
    with urllib.request.urlopen(f'http://127.0.0.1:{port}/json', timeout=2) as response:
        pages = json.load(response)
    for page in pages:
        if page.get('type') != 'page' or not page.get('webSocketDebuggerUrl'):
            continue
        ws = websocket.create_connection(page['webSocketDebuggerUrl'], timeout=3, suppress_origin=True)
        try:
            ws.send(json.dumps({'id': 1, 'method': 'Runtime.evaluate', 'params': {
                'expression': "JSON.stringify({ready:document.readyState,root:!!document.querySelector('#root')?.children.length,text:(document.body?.innerText||'').trim().length})",
                'returnByValue': True}}))
            while True:
                response = json.loads(ws.recv())
                if response.get('id') == 1:
                    value = json.loads(response['result']['result']['value'])
                    if value['ready'] == 'complete' and value['root'] and value['text']:
                        return True
                    break
        finally:
            ws.close()
    return False


def main():
    parser = argparse.ArgumentParser()
    parser.add_argument('--installer', type=Path, default=Path(os.environ['LOCALAPPDATA']) / 'momso-release-prepared/s118-final/momso_0.1.1_x64-setup.exe')
    parser.add_argument('--private-terms-file', type=Path, default=Path(os.environ['LOCALAPPDATA']) / 'momso-s118-private-terms.txt')
    parser.add_argument('--source-root', type=Path, default=ROOT)
    parser.add_argument('--built-exe', type=Path)
    args = parser.parse_args()
    install_file = args.installer.resolve(strict=True)
    terms = args.private_terms_file.resolve(strict=True)
    if not any(line.strip() for line in terms.read_text(encoding='utf8').splitlines()):
        raise ValueError('Private terms file must not be empty')
    terms_sha = hashlib.sha256(terms.read_bytes()).hexdigest()
    receipt_path = install_file.parent / 'momso-release-preparation.json'
    evidence_path = install_file.parent / 'startup-evidence.json'
    report_path = install_file.parent / 'data-mtime-report.json'
    inspect = module('inspect_s118', 'scripts/inspect-product-installer-s118.py')
    release = module('release_s118', 'scripts/desktop-release-s118.py')
    windows = module('window_s118', 'scripts/test-s5b-window.py')
    inspect.ensure_safe_state()
    receipt = json.loads(receipt_path.read_text(encoding='utf8')) if receipt_path.exists() else None
    sha = release.check_installer(install_file, '0.1.1', terms)
    if receipt and (sha != receipt['sha256'] or not receipt.get('inspectedFiles')):
        raise RuntimeError('Installer differs from previously inspected build')
    if receipt is None and args.built_exe is None:
        raise ValueError('Built executable required for a new inspection')
    built_exe = (args.built_exe or ROOT / 'target/release/momso-desktop.exe').resolve(strict=True)
    paths = [Path(os.environ['USERPROFILE']) / 'momso', inspect.Path(os.environ['APPDATA']) / 'app.momso.desktop',
             inspect.Path(os.environ['LOCALAPPDATA']) / 'app.momso.desktop']
    before = snapshot(paths)
    root = Path(tempfile.mkdtemp(prefix='momso-s118-current-user-'))
    installed = root / 'installed'
    with socket.socket() as sock:
        sock.bind(('127.0.0.1', 0))
        port = sock.getsockname()[1]
    env = {k: v for k, v in os.environ.items() if not k.startswith(('MOMSO_', 'TAURI_SIGNING_'))}
    env['WEBVIEW2_ADDITIONAL_BROWSER_ARGUMENTS'] = f'--remote-debugging-port={port}'
    desktop = None
    process = windows.ProcessInfo()
    success = False
    try:
        subprocess.run([str(install_file), '/S', '/NS', '/D=' + str(installed)], env=env, check=True, timeout=180, creationflags=0x08000000)
        stages = [args.source_root / 'apps/alpha/desktop/src-tauri/generated/node', args.source_root / 'apps/alpha/desktop/third-party']
        if (installed / 'node/node.exe').read_bytes() != (stages[0] / 'node.exe').read_bytes():
            raise RuntimeError('Installed Node differs from staged Node')
        if receipt is None:
            count = release.check_bundle(installed, stages, terms, built_exe)
        else:
            count = release.check_tree(installed, terms) + sum(release.check_tree(p, terms) for p in stages)
        subprocess.run(['node', str(ROOT / 'apps/alpha/desktop/scripts/verify-desktop-installer.mjs'),
                        str(installed), str(built_exe), str(stages[0] / 'node.exe'), str(install_file)],
                       check=True, timeout=600, creationflags=0x08000000)
        name = 'momso-s118-current-' + os.urandom(8).hex()
        desktop = windows.user.CreateDesktopW(name, None, None, 0, 0x10000000, None)
        if not desktop:
            raise ctypes.WinError(ctypes.get_last_error())
        exe = installed / 'momso-desktop.exe'
        startup = windows.StartupInfo(); startup.cb, startup.desktop = ctypes.sizeof(startup), name
        block = ctypes.create_unicode_buffer('\0'.join(f'{k}={v}' for k,v in sorted(env.items(), key=lambda x:x[0].upper())) + '\0\0')
        command = ctypes.create_unicode_buffer(subprocess.list2cmdline([str(exe)]))
        if not windows.kernel.CreateProcessW(str(exe), command, None, None, False, 0x08000400, block, None, ctypes.byref(startup), ctypes.byref(process)):
            raise ctypes.WinError(ctypes.get_last_error())
        windows.kernel.CloseHandle(process.thread)
        deadline = time.monotonic() + 180
        shown = False
        while time.monotonic() < deadline:
            if windows.kernel.WaitForSingleObject(process.process, 0) != 258:
                raise RuntimeError('Product exited during startup')
            window = windows.main_window(desktop, process.pid)
            if window:
                windows.assert_responsive(window)
                try:
                    if first_screen(port):
                        shown = True; break
                except (OSError, KeyError, ValueError, websocket.WebSocketException):
                    pass
            time.sleep(.5)
        if not shown:
            raise RuntimeError('First rendered screen could not be verified')
        success = True
    finally:
        if process.process:
            if windows.kernel.WaitForSingleObject(process.process, 0) != 0:
                windows.kernel.TerminateProcess(process.process, 1)
                windows.kernel.WaitForSingleObject(process.process, 10000)
            windows.kernel.CloseHandle(process.process)
        if desktop:
            windows.user.CloseDesktop(desktop)
        uninstaller = installed / 'uninstall.exe'
        if uninstaller.is_file() and not inspect.app_running() and not inspect.run_value_exists() and not inspect.shortcut_exists():
            with winreg.OpenKey(winreg.HKEY_CURRENT_USER, inspect.UNINSTALL) as key:
                location = Path(winreg.QueryValueEx(key, 'InstallLocation')[0].strip('"')).resolve()
            if location != installed.resolve():
                raise RuntimeError('Installation registration changed; retained')
            result = subprocess.run([str(uninstaller), '/S'], env=env, timeout=180, creationflags=0x08000000)
            deadline = time.monotonic() + 60
            while (installed / 'momso-desktop.exe').exists() and time.monotonic() < deadline:
                time.sleep(.25)
            if result.returncode or (installed / 'momso-desktop.exe').exists():
                raise RuntimeError('Temporary uninstall failed')
            if inspect.key_exists(winreg.HKEY_CURRENT_USER, inspect.PRODUCT):
                with winreg.OpenKey(winreg.HKEY_CURRENT_USER, inspect.PRODUCT) as key:
                    location = Path(winreg.QueryValueEx(key, '')[0]).resolve()
                if location != installed.resolve():
                    raise RuntimeError('Product registration changed; retained')
                winreg.DeleteKey(winreg.HKEY_CURRENT_USER, inspect.PRODUCT)
        else:
            raise RuntimeError('Temporary uninstall unsafe; retained')
        after = snapshot(paths)
        changed = {p: {'before': before.get(p), 'after': after.get(p)} for p in before.keys() | after.keys() if before.get(p) != after.get(p)}
        report_path.write_text(json.dumps(changed, ensure_ascii=False, indent=2), encoding='utf8')
        print('Existing data metadata changed:', len(changed))
        print('Temporary directory retained:', root)
    if success:
        if hashlib.sha256(terms.read_bytes()).hexdigest() != terms_sha or hashlib.sha256(install_file.read_bytes()).hexdigest() != sha:
            raise ValueError('Installer or private terms changed during inspection')
        if receipt is None:
            receipt = {'version': '0.1.1', 'installer': install_file.name, 'sha256': sha}
        receipt.update(privateTermsSha256=terms_sha, inspectedFiles=count, startupVerified=False)
        receipt_path.write_text(json.dumps(receipt, indent=2) + '\n', encoding='utf8')
        evidence_path.write_text(json.dumps({'installerSha256': sha, 'isolatedUser': False, 'authorizedCurrentUser': True,
                                            'windowResponsive': True, 'firstScreenRendered': True, 'uninstalled': True}) + '\n', encoding='utf8')
        print('Inspected files:', count, '; rendered screen and uninstall passed.')


if __name__ == '__main__':
    main()
