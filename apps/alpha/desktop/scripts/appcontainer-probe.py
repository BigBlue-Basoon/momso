"""Exercise Windows AppContainer isolation with disposable, synthetic files.

This probe does not start an AI provider or touch an existing archive. It creates
one uniquely named AppContainer profile and temporary directory and removes both.
It is evidence about the Windows boundary only, not product integration.
The process exits nonzero when any prerequisite, including nested execution,
fails; it must not be interpreted as proof that the current app is protected.
"""

import ctypes as c
import ctypes.wintypes as w
import msvcrt
import os
from pathlib import Path
import stat
import subprocess
import tempfile
import uuid

k = c.WinDLL("kernel32", use_last_error=True)
u = c.WinDLL("userenv", use_last_error=True)
a = c.WinDLL("advapi32", use_last_error=True)


class StartupInfo(c.Structure):
    _fields_ = [
        ("cb", w.DWORD), ("reserved", w.LPWSTR), ("desktop", w.LPWSTR),
        ("title", w.LPWSTR), ("x", w.DWORD), ("y", w.DWORD),
        ("x_size", w.DWORD), ("y_size", w.DWORD), ("x_count", w.DWORD),
        ("y_count", w.DWORD), ("fill", w.DWORD), ("flags", w.DWORD),
        ("show", w.WORD), ("reserved_size", w.WORD),
        ("reserved_data", c.POINTER(c.c_byte)), ("stdin", w.HANDLE),
        ("stdout", w.HANDLE), ("stderr", w.HANDLE),
    ]


class StartupInfoEx(c.Structure):
    _fields_ = [("info", StartupInfo), ("attributes", c.c_void_p)]


class SecurityCapabilities(c.Structure):
    _fields_ = [("sid", c.c_void_p), ("capabilities", c.c_void_p),
                ("count", w.DWORD), ("reserved", w.DWORD)]


class ProcessInfo(c.Structure):
    _fields_ = [("process", w.HANDLE), ("thread", w.HANDLE),
                ("process_id", w.DWORD), ("thread_id", w.DWORD)]


u.CreateAppContainerProfile.argtypes = [w.LPCWSTR, w.LPCWSTR, w.LPCWSTR, c.c_void_p, w.DWORD, c.POINTER(c.c_void_p)]
u.CreateAppContainerProfile.restype = c.c_long
u.DeleteAppContainerProfile.argtypes = [w.LPCWSTR]
u.DeleteAppContainerProfile.restype = c.c_long
a.ConvertSidToStringSidW.argtypes = [c.c_void_p, c.POINTER(w.LPWSTR)]
a.ConvertSidToStringSidW.restype = w.BOOL
a.FreeSid.argtypes = [c.c_void_p]
k.LocalFree.argtypes = [c.c_void_p]
k.InitializeProcThreadAttributeList.argtypes = [c.c_void_p, w.DWORD, w.DWORD, c.POINTER(c.c_size_t)]
k.InitializeProcThreadAttributeList.restype = w.BOOL
k.UpdateProcThreadAttribute.argtypes = [c.c_void_p, w.DWORD, c.c_size_t, c.c_void_p, c.c_size_t, c.c_void_p, c.c_void_p]
k.UpdateProcThreadAttribute.restype = w.BOOL
k.DeleteProcThreadAttributeList.argtypes = [c.c_void_p]
k.CreateProcessW.argtypes = [w.LPCWSTR, w.LPWSTR, c.c_void_p, c.c_void_p, w.BOOL, w.DWORD, c.c_void_p, w.LPCWSTR, c.POINTER(StartupInfo), c.POINTER(ProcessInfo)]
k.CreateProcessW.restype = w.BOOL
k.WaitForSingleObject.argtypes = [w.HANDLE, w.DWORD]
k.WaitForSingleObject.restype = w.DWORD
k.GetExitCodeProcess.argtypes = [w.HANDLE, c.POINTER(w.DWORD)]
k.CloseHandle.argtypes = [w.HANDLE]
k.TerminateProcess.argtypes = [w.HANDLE, w.UINT]


def check(ok, step):
    if not ok:
        raise OSError(c.get_last_error(), step)


def acl(*args):
    result = subprocess.run(["icacls", *map(str, args)], capture_output=True,
                            creationflags=0x08000000, check=False)
    if result.returncode:
        raise RuntimeError("temporary ACL configuration failed")


def run():
    name = "MomsoD9Probe" + uuid.uuid4().hex
    sid = c.c_void_p()
    status = u.CreateAppContainerProfile(name, name, name, None, 0, c.byref(sid))
    if status:
        raise OSError(f"profile creation failed: {status & 0xffffffff:#x}")
    print("PROFILE", name, flush=True)
    attributes = None
    initialized = False
    fds = []
    try:
        text = w.LPWSTR()
        check(a.ConvertSidToStringSidW(sid, c.byref(text)), "SID conversion")
        sid_text = text.value
        k.LocalFree(c.cast(text, c.c_void_p))
        workspace = Path(__file__).resolve().parents[4]
        with tempfile.TemporaryDirectory(prefix="momso-d9-probe-", dir=workspace) as tmp:
            root = Path(tmp)
            docs, objects, recovery = (root / name for name in ("zettels", "objects", "recovery"))
            for folder in (docs, objects, recovery):
                folder.mkdir()
            original, saved = objects / "original.txt", recovery / "before.txt"
            ordinary = objects / "ordinary.txt"
            batch, draft = docs / "nested.cmd", docs / "draft.txt"
            original.write_bytes(b"original")
            original.chmod(stat.S_IREAD)
            ordinary.write_bytes(b"ordinary")
            saved.write_bytes(b"preimage")
            draft.write_bytes(b"draft")
            batch.write_text(
                '@echo off\r\necho NESTED_STARTED\r\n'
                f'type "{original}" >NUL 2>&1 && echo ORIGINAL_READ_OK\r\n'
                f'type "{saved}" >NUL 2>&1 && echo RECOVERY_READ_UNEXPECTED\r\n'
                f'attrib "{original}" <NUL >NUL 2>&1 && echo ATTR_TOOL_OK\r\n'
                f'echo ATTR_CLEAR_ATTEMPT\r\nattrib -r "{original}" <NUL >NUL 2>&1 && echo ATTR_EXIT_ZERO\r\n'
                f'echo changed>"{original}"\r\ndel /f "{original}"\r\n'
                f'echo changed>"{ordinary}"\r\ndel /f "{ordinary}"\r\n'
                f'echo LINK_ATTEMPT\r\nmklink /H "{docs / "alias.txt"}" "{ordinary}" >NUL 2>&1 && echo LINK_CREATED\r\n'
                f'if exist "{docs / "alias.txt"}" echo changed>"{docs / "alias.txt"}"\r\n'
                f'echo JUNCTION_ATTEMPT\r\nmklink /J "{docs / "junction"}" "{objects}" >NUL 2>&1 && echo JUNCTION_CREATED\r\n'
                f'if exist "{docs / "junction" / "ordinary.txt"}" echo changed>"{docs / "junction" / "ordinary.txt"}"\r\n'
                f'echo MOVE_ATTEMPT\r\nmove "{objects}" "{docs / "moved-objects"}" >NUL 2>&1 && echo MOVE_SUCCEEDED\r\n'
                f'echo nested>"{draft}"\r\necho changed>"{saved}"\r\ndel /f "{saved}"\r\n',
                encoding="ascii",
            )
            acl(root, "/grant", "*" + sid_text + ":(RX)")
            acl(docs, "/grant", "*" + sid_text + ":(OI)(CI)(M)")
            acl(objects, "/grant", "*" + sid_text + ":(OI)(CI)(RX)")
            acl(docs, "/setintegritylevel", "(OI)(CI)L")
            size = c.c_size_t()
            k.InitializeProcThreadAttributeList(None, 2, 0, c.byref(size))
            buffer = c.create_string_buffer(size.value)
            attributes = c.cast(buffer, c.c_void_p)
            check(k.InitializeProcThreadAttributeList(attributes, 2, 0, c.byref(size)), "attributes")
            initialized = True
            in_read, in_write = os.pipe()
            out_read, out_write = os.pipe()
            fds = [in_read, in_write, out_read, out_write]
            for fd in (in_read, out_write):
                os.set_inheritable(fd, True)
            handles = (w.HANDLE * 2)(msvcrt.get_osfhandle(in_read), msvcrt.get_osfhandle(out_write))
            caps = SecurityCapabilities(sid, None, 0, 0)
            check(k.UpdateProcThreadAttribute(attributes, 0, 0x20009, c.byref(caps), c.sizeof(caps), None, None), "security")
            check(k.UpdateProcThreadAttribute(attributes, 0, 0x20002, c.byref(handles), c.sizeof(handles), None, None), "handles")
            startup = StartupInfoEx()
            startup.info.cb = c.sizeof(startup)
            # A separate hidden console avoids console-host initialization
            # failures in nested console programs without showing a window.
            startup.info.flags = 0x101
            startup.info.show = 0
            startup.info.stdin, startup.info.stdout, startup.info.stderr = handles[0], handles[1], handles[1]
            startup.attributes = attributes
            process = ProcessInfo()
            exe = str(Path(os.environ["SystemRoot"]) / "System32" / "cmd.exe")
            command = c.create_unicode_buffer('"' + exe + '" /d /q')
            check(k.CreateProcessW(exe, command, None, None, True, 0x80000 | 0x00000010,
                                   None, str(root), c.byref(startup.info), c.byref(process)), "process")
            try:
                for fd in (in_read, out_write):
                    os.close(fd)
                    fds.remove(fd)
                commands = (
                    f'type "{batch}" >NUL && echo BATCH_READ_OK\r\n'
                    f'"{Path(os.environ["SystemRoot"]) / "System32" / "whoami.exe"}" >NUL 2>&1 && echo SYSTEM_EXE_OK\r\n'
                    f'"{exe}" /d /c echo CHILD_CMD_OK\r\n'
                    f'"{exe}" /d /q < "{batch}"\r\nexit /b\r\n'
                )
                os.write(in_write, commands.encode("mbcs"))
                os.close(in_write)
                fds.remove(in_write)
                wait = k.WaitForSingleObject(process.process, 20000)
                if wait != 0:
                    k.TerminateProcess(process.process, 1)
                    k.WaitForSingleObject(process.process, 5000)
                    raise TimeoutError("sandbox probe did not finish")
                exit_code = w.DWORD()
                check(k.GetExitCodeProcess(process.process, c.byref(exit_code)), "exit code")
                output = os.read(out_read, 4096)
                print("CHILD_EXIT", exit_code.value,
                      "BATCH_READ", b"BATCH_READ_OK" in output,
                      "SYSTEM_EXE", b"SYSTEM_EXE_OK" in output,
                      "CHILD_CMD", b"CHILD_CMD_OK" in output,
                      "NESTED_STARTED", b"NESTED_STARTED" in output,
                      "ORIGINAL_READ", b"ORIGINAL_READ_OK" in output,
                      "RECOVERY_READ", b"RECOVERY_READ_UNEXPECTED" in output,
                      "ATTR_TOOL", b"ATTR_TOOL_OK" in output,
                      "ATTR_ATTEMPT", b"ATTR_CLEAR_ATTEMPT" in output,
                      "ATTR_EXIT_ZERO", b"ATTR_EXIT_ZERO" in output,
                      "LINK_ATTEMPT", b"LINK_ATTEMPT" in output,
                      "LINK_CREATED", b"LINK_CREATED" in output,
                      "JUNCTION_ATTEMPT", b"JUNCTION_ATTEMPT" in output,
                      "JUNCTION_CREATED", b"JUNCTION_CREATED" in output,
                      "MOVE_ATTEMPT", b"MOVE_ATTEMPT" in output,
                      "MOVE_SUCCEEDED", b"MOVE_SUCCEEDED" in output, flush=True)
                if b"NESTED_STARTED" not in output or draft.read_bytes().strip() != b"nested":
                    print("OUTPUT_ESCAPED", output.decode("cp" + str(k.GetOEMCP()), "replace").encode("unicode_escape").decode("ascii"), flush=True)
                print("DOCUMENT", draft.read_bytes() if draft.exists() else b"MISSING",
                      "ORIGINAL", original.read_bytes() if original.exists() else b"MISSING",
                      "ORDINARY", ordinary.read_bytes() if ordinary.exists() else b"MISSING",
                      "RECOVERY", saved.read_bytes() if saved.exists() else b"MISSING",
                      "ORIGINAL_READONLY", bool(original.stat().st_file_attributes & stat.FILE_ATTRIBUTE_READONLY) if original.exists() else False, flush=True)
                assert b"NESTED_STARTED" in output, "nested process did not start"
                assert b"ORIGINAL_READ_OK" in output, "original was not readable"
                assert b"RECOVERY_READ_UNEXPECTED" not in output, "recovery copy was readable"
                assert b"ATTR_TOOL_OK" in output, "attribute tool did not start"
                assert b"ATTR_CLEAR_ATTEMPT" in output, "attribute clearing was not attempted"
                assert b"LINK_ATTEMPT" in output, "hard-link creation was not attempted"
                assert b"JUNCTION_ATTEMPT" in output, "junction creation was not attempted"
                assert b"MOVE_ATTEMPT" in output, "directory move was not attempted"
                assert original.stat().st_file_attributes & stat.FILE_ATTRIBUTE_READONLY, "read-only attribute changed"
                assert draft.read_bytes().strip() == b"nested", "document was not writable"
                assert original.read_bytes() == b"original", "original was modified or deleted"
                assert ordinary.read_bytes() == b"ordinary", "original without read-only flag was modified or deleted"
                assert objects.is_dir() and not (docs / "moved-objects").exists(), "original directory was moved"
                assert saved.read_bytes() == b"preimage", "recovery copy was modified or deleted"
                print("RESULT: PASS", flush=True)
            finally:
                k.CloseHandle(process.thread)
                k.CloseHandle(process.process)
    finally:
        for fd in fds:
            os.close(fd)
        if initialized:
            k.DeleteProcThreadAttributeList(attributes)
        a.FreeSid(sid)
        status = u.DeleteAppContainerProfile(name)
        print("PROFILE_DELETE", f"{status & 0xffffffff:#x}", name, flush=True)
        if status:
            raise OSError(f"profile cleanup failed: {name}: {status & 0xffffffff:#x}")


if __name__ == "__main__":
    run()
