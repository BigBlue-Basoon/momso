"""Retry only the isolated NSIS removal check, using an already built fixture."""
import ctypes
import importlib.util
import os
from pathlib import Path
import re
import subprocess
import sys
import tempfile
import time

ROOT = Path(__file__).resolve().parent.parent


def main():
    installer = Path(sys.argv[1]).resolve(strict=True)
    bundle = (ROOT / "target/release/bundle/nsis").resolve(strict=True)
    match = re.fullmatch(r"(momso-s118-[0-9a-f]{12})_\d+\.\d+\.\d+_x64-setup\.exe", installer.name)
    if installer.parent != bundle or not match:
        raise ValueError("Expected this worktree's uniquely named S118 fixture")
    name = match.group(1)
    spec = importlib.util.spec_from_file_location("isolated_window", ROOT / "scripts/test-s5b-window.py")
    windows = importlib.util.module_from_spec(spec)
    spec.loader.exec_module(windows)
    with tempfile.TemporaryDirectory(prefix="momso-s69-updater-", ignore_cleanup_errors=True) as temp:
        root = Path(temp)
        env = {k: v for k, v in os.environ.items() if not k.startswith(("TAURI_SIGNING_", "MOMSO_"))}
        env.update(USERPROFILE=str(root / "profile"), HOME=str(root / "profile"),
                   APPDATA=str(root / "profile/AppData/Roaming"), LOCALAPPDATA=str(root / "profile/AppData/Local"),
                   MOMSO_TEST_FAKE_CREDENTIAL_STORE="1", MOMSO_S69_TEST_ROOT=str(root / "runtime"))
        for item in ("OPENAI_API_KEY", "CODEX_API_KEY", "ANTHROPIC_API_KEY", "GEMINI_API_KEY",
                     "GOOGLE_API_KEY", "GH_TOKEN", "GITHUB_TOKEN", "CODEX_HOME", "CLAUDE_CONFIG_DIR",
                     "GH_CONFIG_DIR", "GOOGLE_APPLICATION_CREDENTIALS", "WEBVIEW2_ADDITIONAL_BROWSER_ARGUMENTS"):
            env.pop(item, None)
        Path(env["APPDATA"]).mkdir(parents=True)
        Path(env["LOCALAPPDATA"]).mkdir(parents=True)
        install = root / "installed"
        subprocess.run([str(installer), "/S", "/D=" + str(install)], env=env, check=True, timeout=120, creationflags=0x08000000)
        exe = install / (name + ".exe")
        if not exe.is_file():
            raise RuntimeError("fixture install did not produce the executable")
        desktop = windows.user.CreateDesktopW(name, None, None, 0, 0x10000000, None)
        if not desktop:
            raise ctypes.WinError(ctypes.get_last_error())
        process = windows.ProcessInfo()
        try:
            startup = windows.StartupInfo()
            startup.cb, startup.desktop = ctypes.sizeof(startup), name
            block = ctypes.create_unicode_buffer("\0".join(f"{k}={v}" for k, v in sorted(env.items(), key=lambda pair: pair[0].upper())) + "\0\0")
            command = ctypes.create_unicode_buffer(subprocess.list2cmdline([str(exe)]))
            if not windows.kernel.CreateProcessW(str(exe), command, None, None, False, 0x08000400, block, None, ctypes.byref(startup), ctypes.byref(process)):
                raise ctypes.WinError(ctypes.get_last_error())
            windows.kernel.CloseHandle(process.thread)
            deadline = time.monotonic() + 90
            window = None
            while time.monotonic() < deadline:
                if windows.kernel.WaitForSingleObject(process.process, 0) != 258:
                    raise RuntimeError("fixture exited during startup")
                window = windows.main_window(desktop, process.pid)
                if window:
                    break
                time.sleep(0.2)
            if not window:
                raise RuntimeError("fixture window did not appear")
            windows.assert_responsive(window)
            print("PASS isolated application started with a responsive window", flush=True)
        finally:
            if process.process:
                if windows.kernel.WaitForSingleObject(process.process, 0) != 0:
                    windows.kernel.TerminateProcess(process.process, 1)
                    windows.kernel.WaitForSingleObject(process.process, 10000)
                windows.kernel.CloseHandle(process.process)
            windows.user.CloseDesktop(desktop)
        uninstallers = list(install.glob("*uninstall*.exe"))
        if len(uninstallers) != 1:
            raise RuntimeError("fixture uninstaller not found")
        result = subprocess.run([str(uninstallers[0]), "/S"], env=env, timeout=120, creationflags=0x08000000)
        deadline = time.monotonic() + 60
        while exe.exists() and time.monotonic() < deadline:
            time.sleep(0.25)
        if result.returncode or exe.exists():
            raise RuntimeError(f"fixture removal failed (exit {result.returncode}, executable exists {exe.exists()})")
        print("PASS isolated NSIS removal removed the executable", flush=True)
    if root.exists():
        print("fixture_directory_cleanup_needed=" + str(root), flush=True)


if __name__ == "__main__":
    main()
