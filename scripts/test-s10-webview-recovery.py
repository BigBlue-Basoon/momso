"""Kill only the isolated fixture's WebView2 browser process and verify recovery."""
import ctypes
import importlib.util
import os
from pathlib import Path
import re
import sys
import tempfile
import threading
import time
import uuid

import psutil

sys.dont_write_bytecode = True
spec = importlib.util.spec_from_file_location("s5b_fixture", Path(__file__).with_name("test-s5b-window.py"))
fixture = importlib.util.module_from_spec(spec)
spec.loader.exec_module(fixture)


def read_ready(root, previous_pid=None):
    try:
        data = (root / "ready.txt").read_text(encoding="utf-8")
        app = int(re.search(r"app_pid=(\d+)", data).group(1))
        browser = int(re.search(r"browser_pid=(\d+)", data).group(1))
        if previous_pid == app or browser == 0 or "browser_result=Ok(())" not in data:
            return None
        return app, browser
    except (OSError, AttributeError, ValueError):
        return None


def kill_owned_browser(pid, root):
    process = psutil.Process(pid)
    assert process.name().lower() == "msedgewebview2.exe"
    command = " ".join(process.cmdline()).lower()
    assert str(root / "main-webview").lower() in command, "browser profile is not the isolated fixture"
    process.kill()
    process.wait(timeout=20)


def run_case(executable, hidden, url):
    desktop_name = "momso-s10-" + uuid.uuid4().hex
    desktop = fixture.user.CreateDesktopW(desktop_name, None, None, 0, 0x10000000, None)
    if not desktop:
        raise ctypes.WinError(ctypes.get_last_error())
    original = None
    restarted = None
    root = None
    temporary = tempfile.TemporaryDirectory(prefix="momso-s10-", ignore_cleanup_errors=True)
    try:
        if True:
            tmp = temporary.name
            root = Path(tmp)
            for suffix in ("Local", "Roaming"):
                (root / "user" / "AppData" / suffix).mkdir(parents=True)
            env = os.environ.copy()
            for key in ("WEBVIEW2_ADDITIONAL_BROWSER_ARGUMENTS", "OPENAI_API_KEY", "CODEX_API_KEY", "ANTHROPIC_API_KEY", "GEMINI_API_KEY", "GOOGLE_API_KEY"):
                env.pop(key, None)
            env.update(USERPROFILE=str(root / "user"), LOCALAPPDATA=str(root / "user" / "AppData" / "Local"), APPDATA=str(root / "user" / "AppData" / "Roaming"), MOMSO_S5B_ROOT=str(root), MOMSO_S5B_IDENTIFIER="app.momso.s5b.fixture." + uuid.uuid4().hex, MOMSO_S5B_URL=url, MOMSO_S5B_PANEL="1")
            original = fixture.launch(executable, desktop_name, env)
            app_pid, browser_pid = fixture.wait_until(lambda: read_ready(root))
            assert app_pid == original.pid
            hwnd = fixture.wait_until(lambda: fixture.main_window(desktop, app_pid))
            if hidden:
                assert fixture.user.PostMessageW(hwnd, 0x0010, 0, 0)
                fixture.wait_until(lambda: not fixture.user.IsWindowVisible(hwnd))
            kill_owned_browser(browser_pid, root)
            assert fixture.kernel.WaitForSingleObject(original.process, 30000) == 0, "original app did not restart"
            new_pid, new_browser = fixture.wait_until(lambda: read_ready(root, app_pid), seconds=60)
            restarted = psutil.Process(new_pid)
            assert Path(restarted.exe()).resolve() == Path(executable).resolve()
            new_hwnd = fixture.wait_until(lambda: fixture.main_window(desktop, new_pid))
            assert bool(fixture.user.IsWindowVisible(new_hwnd)) != hidden
            fixture.assert_responsive(new_hwnd)
            # The next immediate failure must not cause a restart loop.
            kill_owned_browser(new_browser, root)
            time.sleep(4)
            assert restarted.is_running() and read_ready(root)[0] == new_pid
            finish = fixture.launch(executable, desktop_name, env, argument="--momso-test-exit")
            try:
                assert fixture.kernel.WaitForSingleObject(finish.process, 15000) == 0
                restarted.wait(timeout=15)
            finally:
                if fixture.kernel.WaitForSingleObject(finish.process, 0) != 0:
                    fixture.kernel.TerminateProcess(finish.process, 2)
                fixture.kernel.CloseHandle(finish.process)
            print(f"hidden={hidden}: recovered, visibility preserved, repeated failure stopped, exit succeeded", flush=True)
            time.sleep(5)
    finally:
        if restarted is None and root is not None:
            try:
                last_pid = int((root / "app-pid.txt").read_text(encoding="utf-8"))
                if original and last_pid != original.pid:
                    candidate = psutil.Process(last_pid)
                    if Path(candidate.exe()).resolve() == Path(executable).resolve() and "--momso-s5b-window-probe" in candidate.cmdline():
                        restarted = candidate
            except (OSError, ValueError, psutil.Error):
                pass
        if restarted is not None and restarted.is_running():
            assert Path(restarted.exe()).resolve() == Path(executable).resolve()
            restarted.kill()
            restarted.wait(timeout=10)
        if original:
            if fixture.kernel.WaitForSingleObject(original.process, 0) != 0:
                fixture.kernel.TerminateProcess(original.process, 2)
                fixture.kernel.WaitForSingleObject(original.process, 5000)
            fixture.kernel.CloseHandle(original.process)
        fixture.user.CloseDesktop(desktop)
        temporary.cleanup()
        if root is not None and root.exists():
            print(f"cleanup_required={root}", flush=True)


if __name__ == "__main__":
    executable = str(Path(os.environ["MOMSO_S5B_EXE"]).resolve(strict=True))
    server = fixture.ThreadingHTTPServer(("127.0.0.1", 0), fixture.FixturePage)
    thread = threading.Thread(target=server.serve_forever, daemon=True)
    thread.start()
    try:
        url = f"http://127.0.0.1:{server.server_port}/"
        run_case(executable, False, url)
        run_case(executable, True, url)
    finally:
        server.shutdown()
        server.server_close()
        thread.join()
