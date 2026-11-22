"""Run an extension popup on an isolated Windows desktop and test its IPC boundary."""
import importlib.util
import os
from pathlib import Path
import queue
import sys
import tempfile
import threading
import uuid
from urllib.parse import parse_qs, urlparse

sys.dont_write_bytecode = True
spec = importlib.util.spec_from_file_location("s5b_fixture", Path(__file__).with_name("test-s5b-window.py"))
fixture = importlib.util.module_from_spec(spec)
spec.loader.exec_module(fixture)
receipts = queue.Queue()


class Page(fixture.FixturePage):
    def do_GET(self):
        if self.path.startswith("/s28-result?"):
            receipts.put(parse_qs(urlparse(self.path).query))
        self.send_response(200)
        self.send_header("Access-Control-Allow-Origin", "*")
        self.send_header("Content-Type", "text/html; charset=utf-8")
        self.end_headers()
        self.wfile.write(b"<!doctype html><title>isolated fixture</title>")


def main():
    executable = str(Path(os.environ["MOMSO_S5B_EXE"]).resolve(strict=True))
    server = fixture.ThreadingHTTPServer(("127.0.0.1", 0), Page)
    thread = threading.Thread(target=server.serve_forever, daemon=True)
    thread.start()
    name = "momso-s28-" + uuid.uuid4().hex
    desktop = fixture.user.CreateDesktopW(name, None, None, 0, 0x10000000, None)
    if not desktop:
        raise OSError("cannot create isolated desktop")
    proc = None
    temporary = tempfile.TemporaryDirectory(prefix="momso-s28-", ignore_cleanup_errors=True)
    root = Path(temporary.name)
    try:
        for suffix in ("Local", "Roaming"):
            (root / "user" / "AppData" / suffix).mkdir(parents=True)
        env = os.environ.copy()
        for key in ("WEBVIEW2_ADDITIONAL_BROWSER_ARGUMENTS", "OPENAI_API_KEY", "CODEX_API_KEY", "ANTHROPIC_API_KEY", "GEMINI_API_KEY", "GOOGLE_API_KEY"):
            env.pop(key, None)
        env.update(USERPROFILE=str(root / "user"), LOCALAPPDATA=str(root / "user" / "AppData" / "Local"), APPDATA=str(root / "user" / "AppData" / "Roaming"), MOMSO_S5B_ROOT=str(root), MOMSO_S5B_IDENTIFIER="app.momso.s5b.fixture." + uuid.uuid4().hex, MOMSO_S5B_URL=f"http://127.0.0.1:{server.server_port}/", MOMSO_S5B_PANEL="1", MOMSO_S28_POPUP="1")
        proc = fixture.launch(executable, name, env)
        ready = fixture.wait_until(lambda: (root / "ready.txt").read_text() if (root / "ready.txt").is_file() else None)
        assert "overlay=Some(Ok(" in ready, ready
        receipt = receipts.get(timeout=30)
        assert len(receipt.get("runtime", [""])[0]) == 32, receipt
        assert receipt.get("ipc") in (["denied"], ["absent"]), receipt
        finish = fixture.launch(executable, name, env, argument="--momso-test-exit")
        try:
            assert fixture.kernel.WaitForSingleObject(finish.process, 15000) == 0
            assert fixture.kernel.WaitForSingleObject(proc.process, 15000) == 0
        finally:
            if fixture.kernel.WaitForSingleObject(finish.process, 0) != 0:
                fixture.kernel.TerminateProcess(finish.process, 2)
            fixture.kernel.CloseHandle(finish.process)
        print("extension popup loaded in isolated profile; extension API works; app IPC denied")
    finally:
        if proc:
            if fixture.kernel.WaitForSingleObject(proc.process, 0) != 0:
                fixture.kernel.TerminateProcess(proc.process, 2)
                fixture.kernel.WaitForSingleObject(proc.process, 5000)
            fixture.kernel.CloseHandle(proc.process)
        fixture.user.CloseDesktop(desktop)
        server.shutdown()
        server.server_close()
        thread.join()
        temporary.cleanup()
        if root.exists():
            print(f"cleanup_required={root}")


if __name__ == "__main__":
    main()
