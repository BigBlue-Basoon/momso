"""Native popover probe on an isolated Windows desktop and local fixture page."""
import ctypes
from ctypes import wintypes
import importlib.util
import json
import os
from pathlib import Path
import sys
import tempfile
import threading
import time
import uuid

sys.dont_write_bytecode = True
spec = importlib.util.spec_from_file_location("s5b_fixture", Path(__file__).with_name("test-s5b-window.py"))
fixture = importlib.util.module_from_spec(spec)
spec.loader.exec_module(fixture)
fixture.user.SetProcessDpiAwarenessContext.argtypes = [ctypes.c_void_p]
fixture.user.SetProcessDpiAwarenessContext.restype = wintypes.BOOL
if not fixture.user.SetProcessDpiAwarenessContext(ctypes.c_void_p(-4)):
    fixture.user.GetThreadDpiAwarenessContext.restype = ctypes.c_void_p
    fixture.user.GetAwarenessFromDpiAwarenessContext.argtypes = [ctypes.c_void_p]
    fixture.user.GetAwarenessFromDpiAwarenessContext.restype = ctypes.c_int
    if fixture.user.GetAwarenessFromDpiAwarenessContext(fixture.user.GetThreadDpiAwarenessContext()) != 2:
        raise ctypes.WinError(ctypes.get_last_error())
fixture.user.GetWindowTextW.argtypes = [wintypes.HWND, wintypes.LPWSTR, ctypes.c_int]
fixture.user.GetWindowRect.argtypes = [wintypes.HWND, ctypes.POINTER(wintypes.RECT)]
fixture.user.GetClientRect.argtypes = [wintypes.HWND, ctypes.POINTER(wintypes.RECT)]
fixture.user.ClientToScreen.argtypes = [wintypes.HWND, ctypes.POINTER(wintypes.POINT)]
fixture.user.SetWindowPos.argtypes = [wintypes.HWND, wintypes.HWND, ctypes.c_int, ctypes.c_int, ctypes.c_int, ctypes.c_int, wintypes.UINT]


class Page(fixture.FixturePage):
    def do_GET(self):
        self.send_response(200)
        self.send_header("Content-Type", "text/html; charset=utf-8")
        self.end_headers()
        self.wfile.write(b'''<!doctype html><title>overlay fixture</title><script>
          window.addEventListener('load', async () => {
            try {
              const state = await window.__TAURI_INTERNALS__.invoke('panel_overlay_state');
              if (state && state.kind === 'fixture')
                await window.__TAURI_INTERNALS__.invoke('overlay_probe_report', {value: state});
            } catch (_) {}
          });
        </script>''')


def overlay_window(desktop, pid):
    found = []
    @fixture.callback
    def visit(hwnd, _):
        name = ctypes.create_unicode_buffer(256)
        owner = wintypes.DWORD()
        fixture.user.GetWindowThreadProcessId(hwnd, ctypes.byref(owner))
        fixture.user.GetWindowTextW(hwnd, name, len(name))
        if owner.value == pid and name.value == "momso":
            found.append(hwnd)
        return True
    fixture.user.EnumDesktopWindows(desktop, visit, 0)
    return found[0] if len(found) == 1 else None


def main_window(desktop, pid, overlay):
    found = []
    @fixture.callback
    def visit(hwnd, _):
        name = ctypes.create_unicode_buffer(256)
        owner = wintypes.DWORD()
        fixture.user.GetClassNameW(hwnd, name, len(name))
        fixture.user.GetWindowThreadProcessId(hwnd, ctypes.byref(owner))
        if owner.value == pid and name.value == "Tauri Window" and hwnd != overlay:
            found.append(hwnd)
        return True
    fixture.user.EnumDesktopWindows(desktop, visit, 0)
    return found[0] if len(found) == 1 else None


def run_case(executable, url, action):
    name = "momso-s13-" + uuid.uuid4().hex
    desktop = fixture.user.CreateDesktopW(name, None, None, 0, 0x10000000, None)
    if not desktop:
        raise ctypes.WinError(ctypes.get_last_error())
    proc = None
    temporary = tempfile.TemporaryDirectory(prefix="momso-s13-", ignore_cleanup_errors=True)
    root = Path(temporary.name)
    try:
        for suffix in ("Local", "Roaming"):
            (root / "user" / "AppData" / suffix).mkdir(parents=True)
        env = os.environ.copy()
        for key in ("WEBVIEW2_ADDITIONAL_BROWSER_ARGUMENTS", "OPENAI_API_KEY", "CODEX_API_KEY", "ANTHROPIC_API_KEY", "GEMINI_API_KEY", "GOOGLE_API_KEY"):
            env.pop(key, None)
        env.update(USERPROFILE=str(root / "user"), LOCALAPPDATA=str(root / "user" / "AppData" / "Local"), APPDATA=str(root / "user" / "AppData" / "Roaming"), MOMSO_S5B_ROOT=str(root), MOMSO_S5B_IDENTIFIER="app.momso.s5b.fixture." + uuid.uuid4().hex, MOMSO_S5B_URL=url, MOMSO_S5B_PANEL="1", MOMSO_S13_OVERLAY="1")
        proc = fixture.launch(executable, name, env)
        ready = fixture.wait_until(lambda: (root / "ready.txt").read_text() if (root / "ready.txt").is_file() else None)
        assert 'overlay=Some(Ok(' in ready, ready
        overlay = fixture.wait_until(lambda: overlay_window(desktop, proc.pid))
        main = fixture.wait_until(lambda: main_window(desktop, proc.pid, overlay))
        assert fixture.user.IsWindowVisible(overlay)
        fixture.user.GetWindow.argtypes = [wintypes.HWND, wintypes.UINT]
        fixture.user.GetWindow.restype = wintypes.HWND
        assert fixture.user.GetWindow(overlay, 4) == main, "overlay is not owned by main window"
        receipt = fixture.wait_until(lambda: json.loads((root / "overlay-invoke.json").read_text()) if (root / "overlay-invoke.json").is_file() else None)
        assert receipt["kind"] == "fixture" and receipt["chatId"] == "s5b-fixture"
        assert receipt["payload"] == {"site": "example.test", "panel": {"width": 320}}
        rect = wintypes.RECT()
        assert fixture.user.GetWindowRect(overlay, ctypes.byref(rect))
        overlay_client = wintypes.RECT()
        overlay_point = wintypes.POINT(0, 0)
        assert fixture.user.GetClientRect(overlay, ctypes.byref(overlay_client))
        assert fixture.user.ClientToScreen(overlay, ctypes.byref(overlay_point))
        point = wintypes.POINT(0, 0)
        assert fixture.user.ClientToScreen(main, ctypes.byref(point))
        fixture.user.GetDpiForWindow.argtypes = [wintypes.HWND]
        fixture.user.GetDpiForWindow.restype = wintypes.UINT
        scale = fixture.user.GetDpiForWindow(main) / 96
        print(f"geometry: overlay=({rect.left},{rect.top},{rect.right},{rect.bottom}) overlay_client=({overlay_point.x},{overlay_point.y},{overlay_client.right},{overlay_client.bottom}) main_client=({point.x},{point.y}) scale={scale}", flush=True)
        assert abs(overlay_point.x - (point.x + 80 * scale)) <= 3
        assert abs(overlay_point.y - (point.y + 70 * scale)) <= 3
        assert abs(overlay_client.right - 320 * scale) <= 3
        assert abs(overlay_client.bottom - 220 * scale) <= 3
        fixture.assert_responsive(main)
        if action == "focus":
            # Simulate the native focus-loss event without moving the real pointer.
            assert fixture.user.PostMessageW(overlay, 0x0086, 1, 0)
            assert fixture.user.PostMessageW(overlay, 0x0007, 0, 0)
            assert fixture.user.PostMessageW(overlay, 0x0008, 0, 0)
        elif action == "move":
            main_rect = wintypes.RECT()
            assert fixture.user.GetWindowRect(main, ctypes.byref(main_rect))
            assert fixture.user.SetWindowPos(main, None, main_rect.left + 2, main_rect.top + 2, 0, 0, 0x0015)
        elif action == "hide":
            assert fixture.user.PostMessageW(main, 0x0010, 0, 0)
        elif action in ("chat", "panel"):
            argument = "--probe-other-chat" if action == "chat" else "--probe-hide-panel"
            switched = fixture.launch(executable, name, env, argument=argument)
            try:
                assert fixture.kernel.WaitForSingleObject(switched.process, 15000) == 0
            finally:
                if fixture.kernel.WaitForSingleObject(switched.process, 0) != 0:
                    fixture.kernel.TerminateProcess(switched.process, 2)
                fixture.kernel.CloseHandle(switched.process)
        fixture.wait_until(lambda: not fixture.user.IsWindowVisible(overlay))
        fixture.assert_responsive(main)
        finish = fixture.launch(executable, name, env, argument="--momso-test-exit")
        try:
            assert fixture.kernel.WaitForSingleObject(finish.process, 15000) == 0
            assert fixture.kernel.WaitForSingleObject(proc.process, 15000) == 0
        finally:
            if fixture.kernel.WaitForSingleObject(finish.process, 0) != 0:
                fixture.kernel.TerminateProcess(finish.process, 2)
            fixture.kernel.CloseHandle(finish.process)
        print(f"{action}: owned overlay placed correctly; app invoke returned state; overlay closed; main responsive", flush=True)
        time.sleep(5)
    finally:
        if proc:
            if fixture.kernel.WaitForSingleObject(proc.process, 0) != 0:
                fixture.kernel.TerminateProcess(proc.process, 2)
                fixture.kernel.WaitForSingleObject(proc.process, 5000)
            fixture.kernel.CloseHandle(proc.process)
        fixture.user.CloseDesktop(desktop)
        temporary.cleanup()
        if root.exists():
            print(f"cleanup_required={root}", flush=True)


if __name__ == "__main__":
    executable = str(Path(os.environ["MOMSO_S5B_EXE"]).resolve(strict=True))
    server = fixture.ThreadingHTTPServer(("127.0.0.1", 0), Page)
    thread = threading.Thread(target=server.serve_forever, daemon=True)
    thread.start()
    try:
        for action in ("focus", "move", "hide", "chat", "panel"):
            run_case(executable, f"http://127.0.0.1:{server.server_port}/", action)
    finally:
        server.shutdown()
        server.server_close()
        thread.join()
