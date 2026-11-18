"""Probe Chromium's Windows CDP pipe with an isolated temporary profile."""
import json
import msvcrt
import os
from pathlib import Path
import queue
import subprocess
import sys
import tempfile
import threading


def probe(executable):
    with tempfile.TemporaryDirectory(prefix="momso-pipe-") as profile:
        child_read, parent_write = os.pipe()
        parent_read, child_write = os.pipe()
        inherited = [msvcrt.get_osfhandle(child_read), msvcrt.get_osfhandle(child_write)]
        for handle in inherited:
            os.set_handle_inheritable(handle, True)
        startup = subprocess.STARTUPINFO()
        startup.lpAttributeList = {"handle_list": inherited}
        arguments = [str(executable), "--headless=new", "--no-first-run", "--disable-background-networking", "--disable-extensions", "--no-startup-window", "--remote-debugging-pipe", f"--remote-debugging-io-pipes={inherited[0]},{inherited[1]}", f"--user-data-dir={profile}"]
        child = None
        try:
            child = subprocess.Popen(arguments, stdin=subprocess.DEVNULL, stdout=subprocess.DEVNULL, stderr=subprocess.DEVNULL, startupinfo=startup, close_fds=True, creationflags=0x08000000)
            os.close(child_read)
            os.close(child_write)
            child_read = child_write = None
            response = queue.Queue()
            def read_message():
                data = bytearray()
                while len(data) < 1024 * 1024:
                    piece = os.read(parent_read, 4096)
                    if not piece:
                        break
                    data.extend(piece)
                    if b"\0" in data:
                        break
                response.put(bytes(data).split(b"\0", 1)[0])
            threading.Thread(target=read_message, daemon=True).start()
            os.write(parent_write, b'{"id":1,"method":"Browser.getVersion"}\0')
            data = response.get(timeout=15)
            message = json.loads(data)
            if message.get("id") != 1 or "result" not in message:
                raise RuntimeError("unexpected_response")
            os.write(parent_write, b'{"id":2,"method":"Browser.close"}\0')
            child.wait(timeout=10)
            return "ok"
        finally:
            if child is not None and child.poll() is None:
                child.kill()
                child.wait()
            for fd in [child_read, child_write, parent_read, parent_write]:
                if fd is not None:
                    os.close(fd)


if __name__ == "__main__":
    if sys.platform != "win32" or len(sys.argv) != 2:
        raise SystemExit("Windows and an executable path are required")
    print(probe(Path(sys.argv[1])))
