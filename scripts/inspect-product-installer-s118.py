"""Inspect an unsigned NSIS in a temporary directory without launching the app."""
import argparse
import ctypes
import os
from pathlib import Path
import shutil
import subprocess
import sys
import tempfile
import time
import winreg

ROOT = Path(__file__).resolve().parent.parent
UNINSTALL = r"Software\Microsoft\Windows\CurrentVersion\Uninstall\momso"
PRODUCT = "Software\\MOW · BigBlue\\momso"
RUN = r"Software\Microsoft\Windows\CurrentVersion\Run"


def key_exists(hive, key):
    try:
        with winreg.OpenKey(hive, key):
            return True
    except FileNotFoundError:
        return False


def run_value_exists():
    try:
        with winreg.OpenKey(winreg.HKEY_CURRENT_USER, RUN) as key:
            winreg.QueryValueEx(key, "momso")
            return True
    except FileNotFoundError:
        return False


def app_running():
    output = subprocess.check_output(["tasklist", "/FI", "IMAGENAME eq momso-desktop.exe", "/FO", "CSV", "/NH"],
                                     text=True, creationflags=0x08000000)
    return '"momso-desktop.exe"' in output.lower()


def shortcut_exists():
    shell = ctypes.WinDLL("shell32", use_last_error=True)
    for folder_id in (0x10, 0x02):  # Desktop and Programs, as used by NSIS.
        path = ctypes.create_unicode_buffer(32768)
        if shell.SHGetFolderPathW(None, folder_id, None, 0, path) != 0:
            raise RuntimeError("Cannot check existing Windows shortcuts")
        if (Path(path.value) / "momso.lnk").exists():
            return True
    return False


def ensure_safe_state():
    for hive in (winreg.HKEY_CURRENT_USER, winreg.HKEY_LOCAL_MACHINE):
        if key_exists(hive, UNINSTALL) or key_exists(hive, PRODUCT):
            raise ValueError("An existing momso installation registration must be preserved")
    if run_value_exists() or app_running() or shortcut_exists():
        raise ValueError("An existing momso process, shortcut or startup entry must be preserved")


def build_target():
    return Path(os.environ.get("CARGO_TARGET_DIR", ROOT / "target")).resolve(strict=True)


def main():
    parser = argparse.ArgumentParser()
    parser.add_argument("--installer", type=Path, required=True)
    parser.add_argument("--private-terms-file", type=Path, required=True)
    args = parser.parse_args()
    installer = args.installer.resolve(strict=True)
    target = build_target()
    bundle = (target / "release/bundle/nsis").resolve(strict=True)
    if installer.parent != bundle or not installer.name.startswith("momso_") or not installer.name.endswith("_x64-setup.exe"):
        raise ValueError("Expected this worktree's product NSIS")
    ensure_safe_state()
    root = Path(tempfile.mkdtemp(prefix="momso-s118-product-inspect-"))
    install = root / "installed"
    env = os.environ.copy()
    env.update(USERPROFILE=str(root / "profile"), HOME=str(root / "profile"),
               APPDATA=str(root / "profile/AppData/Roaming"), LOCALAPPDATA=str(root / "profile/AppData/Local"))
    Path(env["APPDATA"]).mkdir(parents=True)
    Path(env["LOCALAPPDATA"]).mkdir(parents=True)
    completed = False
    try:
        subprocess.run([str(installer), "/S", "/NS", "/D=" + str(install)], env=env, check=True,
                       timeout=180, creationflags=0x08000000)
        if not (install / "momso-desktop.exe").is_file():
            raise RuntimeError("Product did not install in the requested temporary directory")
        command = [sys.executable, str(ROOT / "scripts/desktop-release-s118.py"), "prepare",
                   "--installer", str(installer), "--private-terms-file", str(args.private_terms_file),
                   "--installed-dir", str(install), "--built-exe", str(target / "release/momso-desktop.exe"),
                   "--staged-dir", str(ROOT / "apps/alpha/desktop/src-tauri/generated/node"),
                   "--staged-dir", str(ROOT / "apps/alpha/desktop/third-party")]
        subprocess.run(command, check=True)
    finally:
        uninstaller = install / "uninstall.exe"
        if uninstaller.is_file() and not app_running() and not run_value_exists() and not shortcut_exists():
            with winreg.OpenKey(winreg.HKEY_CURRENT_USER, UNINSTALL) as key:
                location = winreg.QueryValueEx(key, "InstallLocation")[0].strip('"')
            if Path(location).resolve() != install.resolve():
                raise RuntimeError("Installation registration changed; temporary files retained: " + str(root))
            result = subprocess.run([str(uninstaller), "/S"], env=env, timeout=180, creationflags=0x08000000)
            deadline = time.monotonic() + 60
            while (install / "momso-desktop.exe").exists() and time.monotonic() < deadline:
                time.sleep(.25)
            if result.returncode or (install / "momso-desktop.exe").exists():
                raise RuntimeError("Temporary uninstall failed; files retained: " + str(root))
            if key_exists(winreg.HKEY_CURRENT_USER, PRODUCT):
                with winreg.OpenKey(winreg.HKEY_CURRENT_USER, PRODUCT) as key:
                    location = winreg.QueryValueEx(key, "")[0]
                if Path(location).resolve() != install.resolve():
                    raise RuntimeError("Product registration changed; files retained: " + str(root))
                winreg.DeleteKey(winreg.HKEY_CURRENT_USER, PRODUCT)
            completed = True
        elif install.exists():
            raise RuntimeError("Uninstall is unsafe; temporary files retained: " + str(root))
        if completed or not install.exists():
            # NSIS removal can still delete files after its parent process exits.
            deadline = time.monotonic() + 30
            while root.exists() and time.monotonic() < deadline:
                try:
                    shutil.rmtree(root)
                except FileNotFoundError:
                    time.sleep(.25)
                except OSError:
                    time.sleep(.25)
            if root.exists():
                print("temporary_cleanup_needed=" + str(root), file=sys.stderr)
    print("Installed files and staged sources inspected; application was not launched.")


if __name__ == "__main__":
    main()
