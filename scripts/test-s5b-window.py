"""Exercise native close and second-instance activation on a private desktop."""
import ctypes
from ctypes import wintypes
import os
from pathlib import Path
import tempfile
import time
import uuid

user = ctypes.WinDLL("user32", use_last_error=True)
kernel = ctypes.WinDLL("kernel32", use_last_error=True)


class StartupInfo(ctypes.Structure):
    _fields_ = [("cb", wintypes.DWORD), ("reserved", wintypes.LPWSTR), ("desktop", wintypes.LPWSTR), ("title", wintypes.LPWSTR), ("x", wintypes.DWORD), ("y", wintypes.DWORD), ("xsize", wintypes.DWORD), ("ysize", wintypes.DWORD), ("xchars", wintypes.DWORD), ("ychars", wintypes.DWORD), ("fill", wintypes.DWORD), ("flags", wintypes.DWORD), ("show", wintypes.WORD), ("reserved2len", wintypes.WORD), ("reserved2", ctypes.c_void_p), ("stdin", wintypes.HANDLE), ("stdout", wintypes.HANDLE), ("stderr", wintypes.HANDLE)]


class ProcessInfo(ctypes.Structure):
    _fields_ = [("process", wintypes.HANDLE), ("thread", wintypes.HANDLE), ("pid", wintypes.DWORD), ("tid", wintypes.DWORD)]


kernel.CreateProcessW.argtypes = [wintypes.LPCWSTR, wintypes.LPWSTR, ctypes.c_void_p, ctypes.c_void_p, wintypes.BOOL, wintypes.DWORD, ctypes.c_void_p, wintypes.LPCWSTR, ctypes.POINTER(StartupInfo), ctypes.POINTER(ProcessInfo)]
kernel.WaitForSingleObject.argtypes = [wintypes.HANDLE, wintypes.DWORD]
kernel.TerminateProcess.argtypes = [wintypes.HANDLE, wintypes.UINT]
kernel.CloseHandle.argtypes = [wintypes.HANDLE]
user.CreateDesktopW.restype = wintypes.HANDLE
user.CreateDesktopW.argtypes = [wintypes.LPCWSTR, wintypes.LPCWSTR, ctypes.c_void_p, wintypes.DWORD, wintypes.DWORD, ctypes.c_void_p]
user.CloseDesktop.argtypes = [wintypes.HANDLE]
user.GetClassNameW.argtypes = [wintypes.HWND, wintypes.LPWSTR, ctypes.c_int]
user.GetWindowThreadProcessId.argtypes = [wintypes.HWND, ctypes.POINTER(wintypes.DWORD)]
user.PostMessageW.argtypes = [wintypes.HWND, wintypes.UINT, wintypes.WPARAM, wintypes.LPARAM]
user.IsWindowVisible.argtypes = [wintypes.HWND]
user.IsWindowVisible.restype = wintypes.BOOL
callback = ctypes.WINFUNCTYPE(wintypes.BOOL, wintypes.HWND, wintypes.LPARAM)
user.EnumDesktopWindows.argtypes = [wintypes.HANDLE, callback, wintypes.LPARAM]


def main_window(desktop, expected_pid):
    found = []
    @callback
    def visit(hwnd, _):
        name = ctypes.create_unicode_buffer(256)
        user.GetClassNameW(hwnd, name, len(name))
        pid = wintypes.DWORD()
        user.GetWindowThreadProcessId(hwnd, ctypes.byref(pid))
        if pid.value == expected_pid and name.value == "Tauri Window":
            found.append(hwnd)
        return True
    if not user.EnumDesktopWindows(desktop, visit, 0) and ctypes.get_last_error():
        raise ctypes.WinError(ctypes.get_last_error())
    return found[0] if len(found) == 1 else None


def launch(executable, desktop_name, env, finish=False):
    startup = StartupInfo()
    startup.cb = ctypes.sizeof(startup)
    startup.desktop = desktop_name
    proc = ProcessInfo()
    block = ctypes.create_unicode_buffer("\0".join(f"{k}={v}" for k, v in sorted(env.items(), key=lambda x: x[0].upper())) + "\0\0")
    suffix = " --probe-finish" if finish else ""
    command = ctypes.create_unicode_buffer(f'"{executable}" --momso-s5b-window-probe{suffix}')
    if not kernel.CreateProcessW(executable, command, None, None, False, 0x08000400, block, None, ctypes.byref(startup), ctypes.byref(proc)):
        raise ctypes.WinError(ctypes.get_last_error())
    kernel.CloseHandle(proc.thread)
    return proc


def wait_until(check, seconds=40):
    end = time.monotonic() + seconds
    while time.monotonic() < end:
        value = check()
        if value:
            return value
        time.sleep(0.1)
    raise RuntimeError("fixture timed out")


def run_case(executable, with_panel):
    desktop_name = "momso-s5b-" + uuid.uuid4().hex
    desktop = user.CreateDesktopW(desktop_name, None, None, 0, 0x10000000, None)
    if not desktop:
        raise ctypes.WinError(ctypes.get_last_error())
    proc = None
    try:
        with tempfile.TemporaryDirectory(prefix="momso-s5b-", ignore_cleanup_errors=True) as tmp:
            root = Path(tmp)
            for suffix in ("Local", "Roaming"):
                (root / "user" / "AppData" / suffix).mkdir(parents=True)
            env = os.environ.copy()
            for key in ("WEBVIEW2_ADDITIONAL_BROWSER_ARGUMENTS", "OPENAI_API_KEY", "CODEX_API_KEY", "ANTHROPIC_API_KEY", "GEMINI_API_KEY", "GOOGLE_API_KEY"):
                env.pop(key, None)
            env.update(USERPROFILE=str(root / "user"), LOCALAPPDATA=str(root / "user" / "AppData" / "Local"), APPDATA=str(root / "user" / "AppData" / "Roaming"), MOMSO_S5B_ROOT=str(root), MOMSO_S5B_IDENTIFIER="app.momso.s5b.fixture." + uuid.uuid4().hex)
            if with_panel:
                env["MOMSO_S5B_PANEL"] = "1"
            else:
                env.pop("MOMSO_S5B_PANEL", None)
            proc = launch(executable, desktop_name, env)
            ready = wait_until(lambda: (root / "ready.txt").is_file() and (root / "ready.txt").read_text())
            assert "outcome=Ok(())" in ready, ready
            assert f"webview_window={str(not with_panel).lower()}" in ready, ready
            hwnd = wait_until(lambda: main_window(desktop, proc.pid))
            assert user.IsWindowVisible(hwnd)
            assert user.PostMessageW(hwnd, 0x0010, 0, 0)
            wait_until(lambda: not user.IsWindowVisible(hwnd))
            again = launch(executable, desktop_name, env)
            try:
                assert kernel.WaitForSingleObject(again.process, 15000) == 0, "second process did not finish"
                wait_until(lambda: user.IsWindowVisible(hwnd))
            finally:
                if kernel.WaitForSingleObject(again.process, 0) != 0:
                    kernel.TerminateProcess(again.process, 2)
                kernel.CloseHandle(again.process)
            finish = launch(executable, desktop_name, env, finish=True)
            try:
                assert kernel.WaitForSingleObject(finish.process, 15000) == 0, "finish process did not complete"
                assert kernel.WaitForSingleObject(proc.process, 15000) == 0, "fixture did not exit"
            finally:
                if kernel.WaitForSingleObject(finish.process, 0) != 0:
                    kernel.TerminateProcess(finish.process, 2)
                kernel.CloseHandle(finish.process)
            print(f"panel={with_panel}: hidden and reopened; {ready}")
            time.sleep(5)
        if root.exists():
            print(f"cleanup_required={root}")
    finally:
        if proc:
            if kernel.WaitForSingleObject(proc.process, 0) != 0:
                kernel.TerminateProcess(proc.process, 2)
                kernel.WaitForSingleObject(proc.process, 5000)
            kernel.CloseHandle(proc.process)
        user.CloseDesktop(desktop)


if __name__ == "__main__":
    binary = str(Path(os.environ["MOMSO_S5B_EXE"]).resolve(strict=True))
    run_case(binary, False)
    run_case(binary, True)
