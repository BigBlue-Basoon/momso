"""S46 authenticated maintenance gate for the independent updater."""
import fcntl
import json
from pathlib import Path
import time
import urllib.request


class AdmissionError(Exception): pass


class NoRedirect(urllib.request.HTTPRedirectHandler):
    def redirect_request(self, req, fp, code, msg, headers, newurl): return None


class ServerAdmission:
    def __init__(self, token_file, lifecycle_dir, *, port=8765, timeout=120, interval=0.25):
        if not 1 <= int(port) <= 65535 or timeout < 0 or interval <= 0: raise ValueError("invalid_configuration")
        self.token_file = Path(token_file); self.directory = Path(lifecycle_dir)
        self.directory.mkdir(parents=True, exist_ok=True, mode=0o700)
        self.base = f"http://127.0.0.1:{int(port)}"
        self.timeout, self.interval, self.lock = timeout, interval, None
        self.held = False

    def _request(self, path, method="GET"):
        token = self.token_file.read_text(encoding="utf8").strip()
        if not token or "\n" in token or "\r" in token: raise AdmissionError("credential_unavailable")
        request = urllib.request.Request(self.base + path, headers={"Authorization": "Bearer " + token}, method=method, data=b"" if method == "POST" else None)
        opener = urllib.request.build_opener(urllib.request.ProxyHandler({}), NoRedirect())
        with opener.open(request, timeout=5) as response:
            raw = response.read(65537)
        if len(raw) > 65536: raise AdmissionError("invalid_status")
        value = json.loads(raw)
        if not isinstance(value, dict): raise AdmissionError("invalid_status")
        return value

    def health(self):
        return self._request("/health").get("state") == "ready"

    def acquire(self):
        if self.lock is not None: return False
        lock = (self.directory / "lifecycle.lock").open("a+b")
        try: fcntl.flock(lock, fcntl.LOCK_EX | fcntl.LOCK_NB)
        except BlockingIOError:
            lock.close(); return False
        self.lock = lock
        try:
            if (self.directory / "idle-stop.pending").exists():
                self._unlock(); return False
            if self._request("/activity").get("maintenance") is not False:
                self._unlock(); return False
            # Treat lost responses as potentially acquired; keep the lock on errors.
            self.held = True
            if self._request("/maintenance", "POST").get("maintenance") is not True:
                raise AdmissionError("maintenance_unconfirmed")
            deadline = time.monotonic() + self.timeout
            while True:
                state = self._request("/activity")
                if state.get("maintenance") is True and state.get("quiescent") is True and state.get("maintenanceReady") is True:
                    return True
                if state.get("maintenanceReason") == "backup_not_integrated" or time.monotonic() >= deadline:
                    self.release(); return False
                time.sleep(self.interval)
        except Exception:
            # If POST may have reached the server, preserve the lease for recovery.
            if not self.held: self._unlock()
            raise AdmissionError("maintenance_unavailable") from None

    def release(self):
        if not self.held: raise AdmissionError("maintenance_not_owned")
        if self._request("/maintenance/release", "POST").get("maintenance") is not False:
            raise AdmissionError("release_unconfirmed")
        self.held = False
        self._unlock()

    def _unlock(self):
        if self.lock is not None:
            self.lock.close(); self.lock = None
