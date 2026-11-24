"""Reinspect the same unsigned installer after updating private terms, without launching it."""
import hashlib
import importlib.util
import json
import os
from pathlib import Path
import subprocess
import tempfile
import time
import winreg

ROOT = Path(__file__).resolve().parent.parent


def load(name, path):
    spec = importlib.util.spec_from_file_location(name, ROOT / path)
    module = importlib.util.module_from_spec(spec)
    spec.loader.exec_module(module)
    return module


def main():
    dest = Path(os.environ['LOCALAPPDATA']) / 'momso-release-prepared/s118-final'
    installer = dest / 'momso_0.1.1_x64-setup.exe'
    terms = Path(os.environ['LOCALAPPDATA']) / 'momso-s118-private-terms.txt'
    receipt_file = dest / 'momso-release-preparation.json'
    evidence_file = dest / 'startup-evidence.json'
    inspect = load('inspect', 'scripts/inspect-product-installer-s118.py')
    release = load('release', 'scripts/desktop-release-s118.py')
    inspect.ensure_safe_state()
    receipt = json.loads(receipt_file.read_text(encoding='utf8'))
    evidence = json.loads(evidence_file.read_text(encoding='utf8'))
    terms_sha = hashlib.sha256(terms.read_bytes()).hexdigest()
    sha = release.check_installer(installer, '0.1.1', terms)
    if sha != receipt.get('sha256') or sha != evidence.get('installerSha256'):
        raise ValueError('Installer does not match prior inspection and startup evidence')
    if not receipt.get('startupVerified') or not all(evidence.get(x) is True for x in ('authorizedCurrentUser', 'windowResponsive', 'firstScreenRendered', 'uninstalled')):
        raise ValueError('Prior product startup evidence is incomplete')
    stages = [ROOT / 'apps/alpha/desktop/src-tauri/generated/node', ROOT / 'apps/alpha/desktop/third-party']
    root = Path(tempfile.mkdtemp(prefix='momso-s118b-content-'))
    installed = root / 'installed'
    env = os.environ.copy()
    profile = root / 'profile'
    env.update(USERPROFILE=str(profile), HOME=str(profile), APPDATA=str(profile/'AppData/Roaming'), LOCALAPPDATA=str(profile/'AppData/Local'))
    Path(env['APPDATA']).mkdir(parents=True)
    Path(env['LOCALAPPDATA']).mkdir(parents=True)
    count = None
    try:
        subprocess.run([str(installer), '/S', '/NS', '/D=' + str(installed)], check=True, timeout=180, env=env, creationflags=0x08000000)
        if not (installed / 'momso-desktop.exe').is_file():
            raise ValueError('Product executable is missing')
        if (installed / 'node/node.exe').read_bytes() != (stages[0] / 'node.exe').read_bytes():
            raise ValueError('Installed Node differs from staged Node')
        count = release.check_tree(installed, terms) + sum(release.check_tree(p, terms) for p in stages)
    finally:
        uninstaller = installed / 'uninstall.exe'
        if uninstaller.is_file() and not inspect.app_running() and not inspect.run_value_exists() and not inspect.shortcut_exists():
            with winreg.OpenKey(winreg.HKEY_CURRENT_USER, inspect.UNINSTALL) as key:
                actual = Path(winreg.QueryValueEx(key, 'InstallLocation')[0].strip('"')).resolve()
            if actual != installed.resolve():
                raise RuntimeError('Installation registration changed; preserving files')
            result = subprocess.run([str(uninstaller), '/S'], env=env, timeout=180, creationflags=0x08000000)
            deadline = time.monotonic() + 60
            while (installed / 'momso-desktop.exe').exists() and time.monotonic() < deadline:
                time.sleep(.25)
            if result.returncode or (installed / 'momso-desktop.exe').exists():
                raise RuntimeError('Uninstall failed; preserving files')
            if inspect.key_exists(winreg.HKEY_CURRENT_USER, inspect.PRODUCT):
                with winreg.OpenKey(winreg.HKEY_CURRENT_USER, inspect.PRODUCT) as key:
                    location = Path(winreg.QueryValueEx(key, '')[0]).resolve()
                if location != installed.resolve():
                    raise RuntimeError('Product registration changed; preserving files')
                winreg.DeleteKey(winreg.HKEY_CURRENT_USER, inspect.PRODUCT)
        elif installed.exists():
            raise RuntimeError('Uninstall unsafe; preserving files')
        print('Temporary directory retained:', root)
    if count is None:
        raise RuntimeError('Content inspection did not finish')
    if hashlib.sha256(installer.read_bytes()).hexdigest() != sha:
        raise ValueError('Installer changed during inspection')
    if hashlib.sha256(terms.read_bytes()).hexdigest() != terms_sha:
        raise ValueError('Private terms changed during inspection')
    receipt['privateTermsSha256'] = terms_sha
    receipt['inspectedFiles'] = count
    receipt_file.write_text(json.dumps(receipt, indent=2) + '\n', encoding='utf8')
    print('Rescan passed:', count, 'files; prior startup evidence retained.')


if __name__ == '__main__':
    main()
