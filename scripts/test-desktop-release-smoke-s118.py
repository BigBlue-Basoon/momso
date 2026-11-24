"""Install, start and uninstall an isolated unsigned release-mode fixture on Windows."""
import ctypes
import importlib.util
import json
import os
from pathlib import Path
import shutil
import subprocess
import tempfile
import time
import uuid

ROOT = Path(__file__).resolve().parent.parent
DESKTOP = ROOT / "apps/alpha/desktop"
TAURI = ROOT / "node_modules/@tauri-apps/cli/tauri.js"


def load(name, path):
    spec = importlib.util.spec_from_file_location(name, path)
    module = importlib.util.module_from_spec(spec)
    spec.loader.exec_module(module)
    return module


def main():
    if os.name != "nt":
        raise RuntimeError("Windows required")
    updater = load("updater_fixture", ROOT / "scripts/test-desktop-updater-s69.py")
    windows = load("isolated_window", ROOT / "scripts/test-s5b-window.py")
    identity = uuid.uuid4().hex[:12]
    name = "momso-s118-" + identity
    # RuntimePaths accepts only this established fixture prefix for its isolated root.
    with tempfile.TemporaryDirectory(prefix="momso-s69-updater-", ignore_cleanup_errors=True) as temp:
        root = Path(temp)
        env = {k: v for k, v in os.environ.items()
               if not k.startswith(("TAURI_SIGNING_", "MOMSO_"))}
        # The release-like fixture must never use the user's Windows credential store.
        # This test-only feature is compiled only with debug assertions enabled.
        env.update(CARGO_TARGET_DIR=str(ROOT / "target"), CARGO_BUILD_JOBS="1", CARGO_INCREMENTAL="0")
        node = root / "node"
        updater.stage_node(node)
        notices = root / "notices"
        shutil.copytree(DESKTOP / "third-party", notices)
        resources = {"generated/node/": None, "../third-party/": None,
                     node.as_posix() + "/": "node/", notices.as_posix() + "/": "third-party/"}
        resources.update(updater.stage_crt(root, node, env))
        config = {"productName": name, "identifier": "app.momso.fixture." + identity,
                  "mainBinaryName": name, "app": {"windows": [{"label": "main", "visible": False, "focus": False}]},
                  "build": {"beforeBuildCommand": ""},
                  "bundle": {"createUpdaterArtifacts": False, "resources": resources}}
        config_file = root / "config.json"
        config_file.write_text(json.dumps(config), encoding="utf-8")
        subprocess.run(["node", str(TAURI), "build", "--features", "g61-test-probe", "--bundles", "nsis", "--config", str(config_file), "--",
                        "--config", "profile.release.package.momso-desktop.debug-assertions=true", "-j", "1"],
                       cwd=DESKTOP, env=env, check=True)
        version = json.loads((DESKTOP / "src-tauri/tauri.conf.json").read_text(encoding="utf-8"))["version"]
        installer = ROOT / "target/release/bundle/nsis" / f"{name}_{version}_x64-setup.exe"
        if not installer.is_file():
            raise RuntimeError("fixture installer missing")
        profile = root / "profile"
        env.update(USERPROFILE=str(profile), HOME=str(profile), APPDATA=str(profile / "AppData/Roaming"),
                   LOCALAPPDATA=str(profile / "AppData/Local"), MOMSO_TEST_FAKE_CREDENTIAL_STORE="1",
                   MOMSO_S69_TEST_ROOT=str(root / "runtime"))
        for item in ("APPDATA", "LOCALAPPDATA"):
            Path(env[item]).mkdir(parents=True, exist_ok=True)
        for item in ("OPENAI_API_KEY", "CODEX_API_KEY", "ANTHROPIC_API_KEY", "GEMINI_API_KEY",
                     "GOOGLE_API_KEY", "GH_TOKEN", "GITHUB_TOKEN", "CODEX_HOME", "CLAUDE_CONFIG_DIR",
                     "GH_CONFIG_DIR", "GOOGLE_APPLICATION_CREDENTIALS", "WEBVIEW2_ADDITIONAL_BROWSER_ARGUMENTS"):
            env.pop(item, None)
        install = root / "installed"
        process = windows.ProcessInfo()
        desk = None
        try:
            subprocess.run([str(installer), "/S", "/D=" + str(install)], env=env, check=True,
                           timeout=120, creationflags=0x08000000)
            executable = install / (name + ".exe")
            if not executable.is_file() or not (install / "node/node.exe").is_file():
                raise RuntimeError("installed app or Node runtime missing")
            desk = windows.user.CreateDesktopW(name, None, None, 0, 0x10000000, None)
            if not desk:
                raise ctypes.WinError(ctypes.get_last_error())
            startup = windows.StartupInfo()
            startup.cb, startup.desktop = ctypes.sizeof(startup), name
            block = ctypes.create_unicode_buffer("\0".join(f"{k}={v}" for k, v in sorted(env.items(), key=lambda pair: pair[0].upper())) + "\0\0")
            command = ctypes.create_unicode_buffer(subprocess.list2cmdline([str(executable)]))
            if not windows.kernel.CreateProcessW(str(executable), command, None, None, False, 0x08000400,
                                                  block, None, ctypes.byref(startup), ctypes.byref(process)):
                raise ctypes.WinError(ctypes.get_last_error())
            windows.kernel.CloseHandle(process.thread)
            deadline = time.monotonic() + 90
            window = None
            while time.monotonic() < deadline:
                if windows.kernel.WaitForSingleObject(process.process, 0) != 258:
                    raise RuntimeError("isolated app exited during startup")
                window = windows.main_window(desk, process.pid)
                if window:
                    break
                time.sleep(0.2)
            if not window:
                raise RuntimeError("isolated app did not create its main window")
            windows.assert_responsive(window)
            print("PASS isolated unsigned installer installed and responsive application started", flush=True)
        finally:
            if process.process:
                if windows.kernel.WaitForSingleObject(process.process, 0) != 0:
                    windows.kernel.TerminateProcess(process.process, 1)
                    windows.kernel.WaitForSingleObject(process.process, 10000)
                windows.kernel.CloseHandle(process.process)
            if desk:
                windows.user.CloseDesktop(desk)
            uninstallers = list(install.glob("*uninstall*.exe")) if install.exists() else []
            if len(uninstallers) != 1:
                print("fixture_uninstaller_missing=" + str(install), flush=True)
            else:
                result = subprocess.run([str(uninstallers[0]), "/S"], env=env, timeout=120, creationflags=0x08000000)
                # NSIS can launch its removal worker before the initial process exits.
                deadline = time.monotonic() + 60
                while (install / (name + ".exe")).exists() and time.monotonic() < deadline:
                    time.sleep(0.25)
                if result.returncode or (install / (name + ".exe")).exists():
                    raise RuntimeError(f"fixture uninstall failed (exit {result.returncode}): {install}")
                print("PASS isolated fixture uninstalled", flush=True)
            for path in (installer, ROOT / "target/release" / (name + ".exe")):
                try:
                    path.unlink(missing_ok=True)
                except OSError:
                    print("fixture_artifact_cleanup_needed=" + str(path), flush=True)
    if root.exists():
        print("fixture_directory_cleanup_needed=" + str(root), flush=True)


if __name__ == "__main__":
    main()
