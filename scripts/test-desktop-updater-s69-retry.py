"""Verify an invalid signed update cannot change an isolated installed fixture.
Pass its public test config and two signed installers from the S69 fixture build.
"""
import ctypes
import hashlib
import http.server
import importlib.util
import json
import os
from pathlib import Path
import subprocess
import sys
import tempfile
import threading
import time
import urllib.parse

ROOT = Path(__file__).resolve().parent.parent


def main(config_path, first, second, mode="signature"):
    config = json.loads(Path(config_path).read_text(encoding='utf-8'))
    name = config['productName']
    if not name.startswith('momso-s69-'):
        raise ValueError('not an isolated S69 fixture')
    url = urllib.parse.urlsplit(config['plugins']['updater']['endpoints'][0])
    if url.scheme != 'http' or url.hostname != '127.0.0.1':
        raise ValueError('fixture must use loopback')
    first, second = Path(first), Path(second)
    if mode not in ('signature', 'lower', 'malformed', 'installer', 'interrupt', 'power') or not first.name.startswith(name + '_0.1.1_'):
        raise ValueError('fixture installers do not match')
    if mode in ('signature', 'lower', 'malformed') and not second.name.startswith(name + '_0.1.2_'):
        raise ValueError('fixture installers do not match')
    signature_source = first if mode == 'signature' else second
    invalid_signature = Path(str(signature_source) + '.sig').read_text(encoding='utf-8').strip()
    payload = second.read_bytes()
    response = json.dumps({'version': '0.1.0' if mode == 'lower' else '0.1.2', 'platforms': {'windows-x86_64': {'url': f'http://127.0.0.1:{url.port}/installer.exe', 'signature': invalid_signature}}}).encode()
    if mode == 'malformed':
        response = b'{invalid-json'
    requests = {'manifest': 0, 'installer': 0}
    class Handler(http.server.BaseHTTPRequestHandler):
        def log_message(self, *_): pass
        def do_GET(self):
            requests['manifest' if self.path == '/latest.json' else 'installer'] += 1
            value = response if self.path == '/latest.json' else payload
            self.send_response(200); self.send_header('Content-Length', str(len(value))); self.end_headers(); self.wfile.write(value)
    httpd = http.server.ThreadingHTTPServer(('127.0.0.1', url.port), Handler)
    thread = threading.Thread(target=httpd.serve_forever, daemon=True); thread.start()
    spec = importlib.util.spec_from_file_location('fixture', ROOT / 'scripts/test-s5b-window.py')
    fixture = importlib.util.module_from_spec(spec); spec.loader.exec_module(fixture)
    try:
        with tempfile.TemporaryDirectory(prefix='momso-s69-reject-', ignore_cleanup_errors=True) as temp:
            root = Path(temp); install = root / 'installed'; marker = root / 'result.txt'
            env = os.environ.copy()
            for key in list(env):
                if key.startswith('TAURI_SIGNING_') or key in ('OPENAI_API_KEY', 'CODEX_API_KEY', 'ANTHROPIC_API_KEY', 'GEMINI_API_KEY', 'GOOGLE_API_KEY', 'GH_TOKEN', 'GITHUB_TOKEN', 'CODEX_HOME', 'CLAUDE_CONFIG_DIR', 'GOOGLE_APPLICATION_CREDENTIALS', 'GH_CONFIG_DIR', 'WEBVIEW2_ADDITIONAL_BROWSER_ARGUMENTS'):
                    env.pop(key, None)
            env.update(USERPROFILE=str(root / 'user'), APPDATA=str(root / 'user/AppData/Roaming'), LOCALAPPDATA=str(root / 'user/AppData/Local'), TEMP=str(root), TMP=str(root), MOMSO_TEST_LOCAL_UPDATER='1', MOMSO_TEST_LOCAL_UPDATER_FILE=str(marker), MOMSO_TEST_FAKE_CREDENTIAL_STORE='1', MOMSO_S69_TEST_ROOT=str(root / 'runtime'), MOMSO_S69_TEST_EXE=str(install / (name + '.exe')), MOMSO_S69_TEST_NEWFILE=str(install / 's81-new-component.dll'), MOMSO_S69_TEST_INSTALLER_PID=str(root / 'installer-pid.txt'))
            Path(env['APPDATA']).mkdir(parents=True); Path(env['LOCALAPPDATA']).mkdir(parents=True)
            subprocess.run([str(first), '/S', '/D=' + str(install)], env=env, check=True, timeout=120, creationflags=0x08000000)
            exe = install / (name + '.exe')
            before = hashlib.sha256(exe.read_bytes()).digest()
            def matches_prior():
                try:
                    return exe.is_file() and hashlib.sha256(exe.read_bytes()).digest() == before
                except OSError:
                    return False
            def installer_pid():
                try:
                    value = (root / 'installer-pid.txt').read_text(encoding='utf-8').strip()
                    return int(value) if value.isdigit() else None
                except OSError:
                    return None
            desktop_name = name + '-reject'
            desktop = fixture.user.CreateDesktopW(desktop_name, None, None, 0, 0x10000000, None)
            if not desktop: raise ctypes.WinError(ctypes.get_last_error())
            process = fixture.ProcessInfo()
            test_key = None
            snapshot_id = None
            try:
                startup = fixture.StartupInfo(); startup.cb = ctypes.sizeof(startup); startup.desktop = desktop_name
                block = ctypes.create_unicode_buffer('\0'.join(f'{k}={v}' for k,v in sorted(env.items(), key=lambda x:x[0].upper())) + '\0\0')
                command = ctypes.create_unicode_buffer(subprocess.list2cmdline([str(exe)]))
                if not fixture.kernel.CreateProcessW(str(exe), command, None, None, False, 0x08000400, block, None, ctypes.byref(startup), ctypes.byref(process)):
                    raise ctypes.WinError(ctypes.get_last_error())
                fixture.kernel.CloseHandle(process.thread)
                deadline = time.monotonic() + 300
                while time.monotonic() < deadline:
                    state = marker.read_text(encoding='utf-8') if marker.exists() else ''
                    if mode == 'signature' and state == 'download_failed': break
                    if mode in ('lower', 'malformed') and state == 'check_failed': break
                    if mode == 'installer' and state == 'install_failed:Ok(\"failed\")': break
                    if mode in ('interrupt', 'power') and installer_pid() is not None: break
                    if state and state != 'installing': raise RuntimeError('unexpected result: ' + state)
                    time.sleep(1)
                else: raise RuntimeError('signature rejection timed out')
                if mode in ('interrupt', 'power'):
                    import psutil
                    pid = installer_pid()
                    if pid is None: raise RuntimeError('fixture installer PID missing')
                    installer = psutil.Process(pid)
                    installer_path = Path(installer.exe()).resolve()
                    if not installer_path.is_relative_to(root.resolve()) or hashlib.sha256(installer_path.read_bytes()).digest() != hashlib.sha256(payload).digest() or '/UPDATE' not in installer.cmdline():
                        raise RuntimeError('refusing to stop a process outside the isolated fixture')
                    if mode == 'power':
                        snapshot_root = root / 'runtime/data/backups/update-recovery'
                        snapshots = list(snapshot_root.glob('*/files/' + name + '.exe'))
                        if len(snapshots) != 1: raise RuntimeError('fixture recovery helper missing')
                        helper = snapshots[0].resolve()
                        import winreg
                        snapshot_id = helper.parent.parent.name
                        test_key = 'Software\\momso\\s81-tests\\' + snapshot_id
                        with winreg.OpenKey(winreg.HKEY_CURRENT_USER, test_key) as handle:
                            registered = winreg.QueryValueEx(handle, 'momso-update-recovery-' + snapshot_id)[0]
                        suffix = '" --momso-update-recover'
                        if not registered.startswith('"') or not registered.endswith(suffix) or Path(registered[1:-len(suffix)]).resolve() != helper:
                            raise RuntimeError('wrong recovery registration')
                        supervisors = []
                        for candidate in psutil.process_iter(['pid', 'exe', 'cmdline']):
                            try:
                                if candidate.info['exe'] and Path(candidate.info['exe']).resolve() == helper and '--momso-update-supervisor' in (candidate.info['cmdline'] or []): supervisors.append(candidate)
                            except (psutil.NoSuchProcess, psutil.AccessDenied): pass
                        if len(supervisors) != 1: raise RuntimeError('fixture supervisor not uniquely identified')
                        supervisors[0].kill(); supervisors[0].wait(timeout=15)
                        def run_helper(timeout_ms):
                            info = fixture.ProcessInfo()
                            helper_command = ctypes.create_unicode_buffer(subprocess.list2cmdline([str(helper), '--momso-update-recover']))
                            if not fixture.kernel.CreateProcessW(str(helper), helper_command, None, None, False, 0x08000400, block, None, ctypes.byref(startup), ctypes.byref(info)): raise ctypes.WinError(ctypes.get_last_error())
                            fixture.kernel.CloseHandle(info.thread)
                            if fixture.kernel.WaitForSingleObject(info.process, timeout_ms) != 0: raise RuntimeError('fixture helper did not finish')
                            fixture.kernel.CloseHandle(info.process)
                        run_helper(30000)
                        if matches_prior(): raise RuntimeError('recovered while installer was still running')
                    installer.kill()
                    installer.wait(timeout=15)
                    if mode == 'power': run_helper(240000)
                    deadline = time.monotonic() + 180
                    while time.monotonic() < deadline:
                        if matches_prior():
                            break
                        time.sleep(0.5)
                assert matches_prior(), 'installed app changed'
                if mode in ('lower', 'malformed') and (requests['manifest'] < 1 or requests['installer'] != 0):
                    raise RuntimeError('invalid release unexpectedly downloaded installer')
                if mode in ('interrupt', 'power'):
                    deadline = time.monotonic() + 180
                    while time.monotonic() < deadline:
                        if marker.exists() and marker.read_text(encoding='utf-8') == 'restored:0.1.1':
                            break
                        time.sleep(1)
                    else:
                        raise RuntimeError('restored app did not restart')
                    if (install / 's81-new-component.dll').exists(): raise RuntimeError('new installation file survived rollback')
                    if mode == 'power':
                        deadline = time.monotonic() + 90
                        while time.monotonic() < deadline:
                            try:
                                handle = winreg.OpenKey(winreg.HKEY_CURRENT_USER, test_key)
                            except FileNotFoundError:
                                break
                            else:
                                handle.Close()
                            time.sleep(0.5)
                        else: raise RuntimeError('fixture recovery registration was not cleaned')
                    time.sleep(2)
                print('PASS real isolated app rejected ' + mode + ' failure and preserved 0.1.1 executable', flush=True)
            finally:
                if process.process:
                    if fixture.kernel.WaitForSingleObject(process.process, 0) != 0: fixture.kernel.TerminateProcess(process.process, 1)
                    fixture.kernel.WaitForSingleObject(process.process, 10000); fixture.kernel.CloseHandle(process.process)
                # Clean only this uniquely named fixture's processes, even on an assertion failure.
                import psutil
                for candidate in psutil.process_iter(['pid', 'exe', 'cmdline']):
                    try:
                        if not candidate.info['exe']: continue
                        candidate_path = Path(candidate.info['exe']).resolve()
                        if not candidate_path.is_relative_to(root.resolve()): continue
                        args = candidate.info['cmdline'] or []
                        is_helper = candidate_path.name == name + '.exe' and any(flag in args for flag in ('--momso-update-supervisor', '--momso-update-recover'))
                        is_installer = candidate_path.name == name + '-installer.exe' and hashlib.sha256(candidate_path.read_bytes()).digest() == hashlib.sha256(payload).digest()
                        if is_helper or is_installer:
                            candidate.kill(); candidate.wait(timeout=15)
                    except (psutil.NoSuchProcess, psutil.AccessDenied, OSError): pass
                fixture.user.CloseDesktop(desktop)
                if test_key and snapshot_id:
                    import winreg
                    try:
                        with winreg.OpenKey(winreg.HKEY_CURRENT_USER, test_key, 0, winreg.KEY_SET_VALUE) as handle:
                            try: winreg.DeleteValue(handle, 'momso-update-recovery-' + snapshot_id)
                            except FileNotFoundError: pass
                        winreg.DeleteKey(winreg.HKEY_CURRENT_USER, test_key)
                    except FileNotFoundError: pass
                uninstallers = list(install.glob('*uninstall*.exe')) if install.exists() else []
                if len(uninstallers) == 1:
                    subprocess.run([str(uninstallers[0]), '/S'], env=env, timeout=120, creationflags=0x08000000, check=True)
                elif install.exists(): print('fixture_uninstall_missing=' + name, flush=True)
                time.sleep(5)
        if root.exists(): print('fixture_cleanup_needed=' + str(root), flush=True)
    finally:
        httpd.shutdown(); httpd.server_close(); thread.join(timeout=5)


if __name__ == '__main__': main(*sys.argv[1:])
