"""Run the already-built S7 WebView tests on an unseen Windows desktop."""
import ctypes
from ctypes import wintypes
import os
import subprocess
import sys
import tempfile
import threading
import time
import uuid

user = ctypes.WinDLL("user32", use_last_error=True)
kernel = ctypes.WinDLL("kernel32", use_last_error=True)


class SecurityAttributes(ctypes.Structure):
    _fields_ = [("length", wintypes.DWORD), ("descriptor", ctypes.c_void_p), ("inherit", wintypes.BOOL)]


class StartupInfo(ctypes.Structure):
    _fields_ = [("cb", wintypes.DWORD), ("reserved", wintypes.LPWSTR), ("desktop", wintypes.LPWSTR), ("title", wintypes.LPWSTR), ("x", wintypes.DWORD), ("y", wintypes.DWORD), ("xsize", wintypes.DWORD), ("ysize", wintypes.DWORD), ("xchars", wintypes.DWORD), ("ychars", wintypes.DWORD), ("fill", wintypes.DWORD), ("flags", wintypes.DWORD), ("show", wintypes.WORD), ("reserved2len", wintypes.WORD), ("reserved2", ctypes.c_void_p), ("stdin", wintypes.HANDLE), ("stdout", wintypes.HANDLE), ("stderr", wintypes.HANDLE)]


class ProcessInfo(ctypes.Structure):
    _fields_ = [("process", wintypes.HANDLE), ("thread", wintypes.HANDLE), ("pid", wintypes.DWORD), ("tid", wintypes.DWORD)]


user.CreateDesktopW.argtypes = [wintypes.LPCWSTR, wintypes.LPCWSTR, ctypes.c_void_p, wintypes.DWORD, wintypes.DWORD, ctypes.c_void_p]
user.CreateDesktopW.restype = wintypes.HANDLE
user.CloseDesktop.argtypes = [wintypes.HANDLE]
kernel.CreatePipe.argtypes = [ctypes.POINTER(wintypes.HANDLE), ctypes.POINTER(wintypes.HANDLE), ctypes.POINTER(SecurityAttributes), wintypes.DWORD]
kernel.SetHandleInformation.argtypes = [wintypes.HANDLE, wintypes.DWORD, wintypes.DWORD]
kernel.CreateProcessW.argtypes = [wintypes.LPCWSTR, wintypes.LPWSTR, ctypes.c_void_p, ctypes.c_void_p, wintypes.BOOL, wintypes.DWORD, ctypes.c_void_p, wintypes.LPCWSTR, ctypes.POINTER(StartupInfo), ctypes.POINTER(ProcessInfo)]
kernel.WaitForSingleObject.argtypes = [wintypes.HANDLE, wintypes.DWORD]
kernel.GetExitCodeProcess.argtypes = [wintypes.HANDLE, ctypes.POINTER(wintypes.DWORD)]
kernel.TerminateProcess.argtypes = [wintypes.HANDLE, wintypes.UINT]
kernel.CloseHandle.argtypes = [wintypes.HANDLE]
kernel.ReadFile.argtypes = [wintypes.HANDLE, ctypes.c_void_p, wintypes.DWORD, ctypes.POINTER(wintypes.DWORD), ctypes.c_void_p]


def main():
    executable = os.path.abspath(sys.argv[1])
    desktop_name = "momso-s7-" + uuid.uuid4().hex
    desktop = user.CreateDesktopW(desktop_name, None, None, 0, 0x10000000, None)
    if not desktop:
        raise ctypes.WinError(ctypes.get_last_error())
    # Cleanup is handed off after the workspace's recursive-delete policy rejection.
    owned_temp = tempfile.mkdtemp(prefix="momso-s7-")
    read, write = wintypes.HANDLE(), wintypes.HANDLE()
    process = ProcessInfo()
    try:
        attrs = SecurityAttributes(ctypes.sizeof(SecurityAttributes), None, True)
        if not kernel.CreatePipe(ctypes.byref(read), ctypes.byref(write), ctypes.byref(attrs), 0):
            raise ctypes.WinError(ctypes.get_last_error())
        if not kernel.SetHandleInformation(read, 1, 0):
            raise ctypes.WinError(ctypes.get_last_error())
        startup = StartupInfo()
        startup.cb, startup.desktop = ctypes.sizeof(startup), desktop_name
        startup.flags, startup.show = 0x101, 0
        startup.stdout = startup.stderr = write
        env = os.environ.copy()
        env["TEMP"] = env["TMP"] = owned_temp
        env["MOMSO_S7_SCAN_ROOT"] = owned_temp
        test_secret = uuid.uuid4().hex
        env["MOMSO_S7_TEST_SECRET"] = test_secret
        user_root = os.path.join(owned_temp, "user")
        env["USERPROFILE"] = user_root
        env["LOCALAPPDATA"] = os.path.join(user_root, "AppData", "Local")
        env["APPDATA"] = os.path.join(user_root, "AppData", "Roaming")
        os.makedirs(env["LOCALAPPDATA"], exist_ok=True)
        os.makedirs(env["APPDATA"], exist_ok=True)
        env["MOMSO_TEST_FAKE_CREDENTIAL_STORE"] = "1"
        for key in ("OPENAI_API_KEY", "CODEX_API_KEY", "ANTHROPIC_API_KEY", "GEMINI_API_KEY", "GOOGLE_API_KEY", "GH_TOKEN", "GITHUB_TOKEN", "AWS_ACCESS_KEY_ID", "AWS_SECRET_ACCESS_KEY", "CLOUDFLARE_API_TOKEN", "WEBVIEW2_ADDITIONAL_BROWSER_ARGUMENTS", "MOMSO_SAFE_DIAGNOSTICS"):
            env.pop(key, None)
        block = ctypes.create_unicode_buffer("\0".join(f"{k}={v}" for k, v in sorted(env.items(), key=lambda item: item[0].upper())) + "\0\0")
        test_name = sys.argv[2] if len(sys.argv) > 2 else "saved_login_"
        command = ctypes.create_unicode_buffer(subprocess.list2cmdline([executable, test_name, "--ignored", "--test-threads=1"]))
        if not kernel.CreateProcessW(executable, command, None, None, True, 0x08000400, block, None, ctypes.byref(startup), ctypes.byref(process)):
            raise ctypes.WinError(ctypes.get_last_error())
        kernel.CloseHandle(write)
        write = wintypes.HANDLE()
        chunks = []
        def collect():
            data = ctypes.create_string_buffer(4096)
            count = wintypes.DWORD()
            while kernel.ReadFile(read, data, len(data), ctypes.byref(count), None) and count.value:
                chunks.append(data.raw[:count.value])
        reader = threading.Thread(target=collect, daemon=True)
        reader.start()
        if kernel.WaitForSingleObject(process.process, 360000) != 0:
            kernel.TerminateProcess(process.process, 1)
            kernel.WaitForSingleObject(process.process, 10000)
            raise RuntimeError("isolated fixture timed out")
        result = wintypes.DWORD()
        if not kernel.GetExitCodeProcess(process.process, ctypes.byref(result)):
            raise ctypes.WinError(ctypes.get_last_error())
        reader.join(timeout=10)
        if result.value == 0 and test_name in ("saved_login_auto_through_mcp_and_local_site", "saved_login_ask_through_mcp_and_local_site"):
            deadline = time.monotonic() + 30
            for folder, _, files in os.walk(owned_temp, onerror=lambda error: (_ for _ in ()).throw(error)):
                for name in files:
                    path = os.path.join(folder, name)
                    while True:
                        try:
                            with open(path, "rb") as item:
                                contents = item.read()
                            break
                        except FileNotFoundError:
                            contents = b""
                            break
                        except OSError:
                            if time.monotonic() >= deadline:
                                raise RuntimeError("fixture file unreadable after process exit")
                            time.sleep(0.2)
                    if test_secret.encode() in contents:
                        raise RuntimeError("test secret found in fixture files")
            print("fixture file secret scan: zero matches")
        print(b"".join(chunks).decode("utf-8", "replace"))
        return result.value
    finally:
        if process.process:
            code = wintypes.DWORD()
            if kernel.GetExitCodeProcess(process.process, ctypes.byref(code)) and code.value == 259:
                kernel.TerminateProcess(process.process, 1)
                kernel.WaitForSingleObject(process.process, 10000)
        for handle in (process.thread, process.process, read, write):
            if handle:
                kernel.CloseHandle(handle)
        user.CloseDesktop(desktop)
        print(f"cleanup_required={owned_temp}", file=sys.stderr)


if __name__ == "__main__":
    sys.exit(main())
