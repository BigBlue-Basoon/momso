"""Run the real installer only in a disposable, separate Windows user profile."""
import argparse
import csv
import ctypes
import hashlib
import importlib.util
import json
import os
from pathlib import Path
import re
import shutil
import subprocess
import tempfile
import time
import winreg

ROOT = Path(__file__).resolve().parent.parent


def load(name, path):
    spec = importlib.util.spec_from_file_location(name, path)
    module = importlib.util.module_from_spec(spec)
    spec.loader.exec_module(module)
    return module


def known_folder(folder_id):
    path = ctypes.create_unicode_buffer(32768)
    if ctypes.WinDLL("shell32").SHGetFolderPathW(None, folder_id, None, 0, path) != 0:
        raise RuntimeError("Cannot resolve Windows user folders")
    return Path(path.value)


def require_distinct_sid(account, original_sid):
    row = next(csv.reader([account.strip()]))
    if len(row) != 2 or not re.fullmatch(r"S-1-(?:\d+-)+\d+", original_sid, re.I):
        raise ValueError("A valid original Windows user SID is required")
    if original_sid.lower() == row[1].lower():
        raise ValueError("Use a separate disposable Windows user")


def main():
    parser = argparse.ArgumentParser()
    parser.add_argument("--installer", type=Path, required=True)
    parser.add_argument("--evidence", type=Path, required=True)
    parser.add_argument("--original-user-sid", required=True)
    args = parser.parse_args()
    if os.name != "nt":
        raise RuntimeError("Windows required")
    # The caller supplies the main user's SID; don't record or print either SID.
    account = subprocess.check_output(["whoami", "/user", "/fo", "csv", "/nh"], text=True,
                                      creationflags=0x08000000)
    require_distinct_sid(account, args.original_user_sid)
    installer = args.installer.resolve(strict=True)
    if not installer.name.startswith("momso_") or not installer.name.endswith("_x64-setup.exe"):
        raise ValueError("Expected a product installer")
    data_dirs = [known_folder(0x1A) / "app.momso.desktop", known_folder(0x1C) / "app.momso.desktop",
                 known_folder(0x28) / "momso"]
    if any(path.exists() for path in data_dirs):
        raise ValueError("Disposable user's momso data directories must initially be absent")
    inspect = load("inspect_product", ROOT / "scripts/inspect-product-installer-s118.py")
    windows = load("isolated_window", ROOT / "scripts/test-s5b-window.py")
    inspect.ensure_safe_state()
    root = Path(tempfile.mkdtemp(prefix="momso-s118-startup-"))
    install = root / "installed"
    env = {key: value for key, value in os.environ.items() if not key.startswith(("MOMSO_", "TAURI_SIGNING_"))}
    for item in ("OPENAI_API_KEY", "CODEX_API_KEY", "ANTHROPIC_API_KEY", "GEMINI_API_KEY", "GOOGLE_API_KEY",
                 "GH_TOKEN", "GITHUB_TOKEN", "CODEX_HOME", "CLAUDE_CONFIG_DIR", "GH_CONFIG_DIR", "GOOGLE_APPLICATION_CREDENTIALS"):
        env.pop(item, None)
    desktop = None
    process = windows.ProcessInfo()
    completed = False
    try:
        subprocess.run([str(installer), "/S", "/NS", "/D=" + str(install)], env=env, check=True,
                       timeout=180, creationflags=0x08000000)
        exe = install / "momso-desktop.exe"
        if not exe.is_file():
            raise RuntimeError("Product installation did not complete")
        desktop_name = "momso-s118-product-test"
        desktop = windows.user.CreateDesktopW(desktop_name, None, None, 0, 0x10000000, None)
        if not desktop:
            raise ctypes.WinError(ctypes.get_last_error())
        startup = windows.StartupInfo()
        startup.cb, startup.desktop = ctypes.sizeof(startup), desktop_name
        block = ctypes.create_unicode_buffer("\0".join(f"{k}={v}" for k, v in sorted(env.items(), key=lambda pair: pair[0].upper())) + "\0\0")
        command = ctypes.create_unicode_buffer(subprocess.list2cmdline([str(exe)]))
        if not windows.kernel.CreateProcessW(str(exe), command, None, None, False, 0x08000400, block, None, ctypes.byref(startup), ctypes.byref(process)):
            raise ctypes.WinError(ctypes.get_last_error())
        windows.kernel.CloseHandle(process.thread)
        deadline = time.monotonic() + 120
        window = None
        while time.monotonic() < deadline:
            if windows.kernel.WaitForSingleObject(process.process, 0) != 258:
                raise RuntimeError("Product exited during startup")
            window = windows.main_window(desktop, process.pid)
            if window:
                break
            time.sleep(.2)
        if not window:
            raise RuntimeError("Product window did not appear")
        windows.assert_responsive(window)
        completed = True
    finally:
        if process.process:
            if windows.kernel.WaitForSingleObject(process.process, 0) != 0:
                windows.kernel.TerminateProcess(process.process, 1)
                windows.kernel.WaitForSingleObject(process.process, 10000)
            windows.kernel.CloseHandle(process.process)
        if desktop:
            windows.user.CloseDesktop(desktop)
        uninstaller = install / "uninstall.exe"
        if uninstaller.is_file() and not inspect.app_running() and not inspect.run_value_exists():
            with winreg.OpenKey(winreg.HKEY_CURRENT_USER, inspect.UNINSTALL) as key:
                location = winreg.QueryValueEx(key, "InstallLocation")[0].strip('"')
            if Path(location).resolve() != install.resolve():
                raise RuntimeError("Test installation registration changed; retained: " + str(root))
            result = subprocess.run([str(uninstaller), "/S"], env=env, timeout=180, creationflags=0x08000000)
            deadline = time.monotonic() + 60
            while (install / "momso-desktop.exe").exists() and time.monotonic() < deadline:
                time.sleep(.25)
            if result.returncode or (install / "momso-desktop.exe").exists():
                raise RuntimeError("Test uninstall failed; retained: " + str(root))
            if inspect.key_exists(winreg.HKEY_CURRENT_USER, inspect.PRODUCT):
                with winreg.OpenKey(winreg.HKEY_CURRENT_USER, inspect.PRODUCT) as key:
                    location = winreg.QueryValueEx(key, "")[0]
                if Path(location).resolve() != install.resolve():
                    raise RuntimeError("Test product registration changed; retained: " + str(root))
                winreg.DeleteKey(winreg.HKEY_CURRENT_USER, inspect.PRODUCT)
        elif install.exists():
            raise RuntimeError("Test uninstall is unsafe; retained: " + str(root))
        # A disposable account is required and the paths were verified absent before launch.
        for path in data_dirs:
            if path.exists():
                if path.is_symlink() or path.is_junction():
                    raise RuntimeError("Test data path changed; retained: " + str(path))
                shutil.rmtree(path)
        if root.exists():
            shutil.rmtree(root, ignore_errors=True)
        if root.exists():
            raise RuntimeError("Test files retained: " + str(root))
    if completed:
        args.evidence.write_text(json.dumps({"installerSha256": hashlib.sha256(installer.read_bytes()).hexdigest(),
                                             "isolatedUser": True, "windowResponsive": True, "uninstalled": True}) + "\n", encoding="utf-8")
        print("Isolated product startup and uninstall passed.")


if __name__ == "__main__":
    main()
