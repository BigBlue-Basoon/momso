"""S46 authenticated maintenance gate for the independent updater."""
import fcntl
import json
import os
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
        self.closed_at = None
        # Written before POST /maintenance and removed only after a confirmed
        # release. It proves that a closed gate found later was closed by a
        # ServerAdmission owner (all owners hold lifecycle.lock), so a lost
        # response never leaves admission closed with nobody able to reopen it.
        self.marker = self.directory / "admission.pending"

    def pending(self):
        return self.marker.exists()

    def _sync_directory(self):
        fd = os.open(self.directory, os.O_RDONLY | os.O_DIRECTORY)
        try: os.fsync(fd)
        finally: os.close(fd)

    def _write_marker(self):
        if self.marker.exists(): return
        fd = os.open(self.marker, os.O_WRONLY | os.O_CREAT | os.O_TRUNC | os.O_NOFOLLOW, 0o600)
        try:
            os.write(fd, b"admission\n"); os.fsync(fd)
        finally:
            os.close(fd)
        self._sync_directory()

    def _remove_marker(self):
        if not self.marker.exists(): return
        self.marker.unlink(missing_ok=True)
        self._sync_directory()

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
        if self.lock is not None:
            # A failed earlier acquire or release in this process kept the
            # lock because the gate may be closed; continue as its owner.
            if not self.held: return False
        else:
            lock = (self.directory / "lifecycle.lock").open("a+b")
            try: fcntl.flock(lock, fcntl.LOCK_EX | fcntl.LOCK_NB)
            except BlockingIOError:
                lock.close(); return False
            self.lock = lock
        try:
            if (self.directory / "idle-stop.pending").exists():
                if not self.held: self._unlock()
                return False
            maintenance = self._request("/activity").get("maintenance")
            if self.pending() and maintenance is True:
                # Our earlier POST reached the server but its answer was lost.
                self.held = True
                self.closed_at = None
            else:
                if self.pending() and maintenance is False:
                    self.held = False
                    self._remove_marker()
                if maintenance is not False:
                    if not self.held: self._unlock()
                    return False
                # Treat lost responses as potentially acquired; keep the lock on errors.
                self._write_marker()
                self.held = True
                response = self._request("/maintenance", "POST")
                if response.get("maintenance") is not True:
                    raise AdmissionError("maintenance_unconfirmed")
                self.closed_at = response.get("closedAt")
            deadline = time.monotonic() + self.timeout
            while True:
                state = self._request("/activity")
                if state.get("maintenance") is True and state.get("quiescent") is True:
                    return True
                if time.monotonic() >= deadline:
                    self.release(); return False
                time.sleep(self.interval)
        except Exception:
            # If POST may have reached the server, preserve the lease for recovery.
            if not self.held: self._unlock()
            raise AdmissionError("maintenance_unavailable") from None

    def reacquire_for_recovery(self):
        """Only an updater with its durable switching record may call this."""
        if self.lock is not None:
            if not self.held: return False
        else:
            lock = (self.directory / "lifecycle.lock").open("a+b")
            try:
                fcntl.flock(lock, fcntl.LOCK_EX | fcntl.LOCK_NB)
            except BlockingIOError:
                lock.close(); return False
            self.lock = lock
        try:
            if (self.directory / "idle-stop.pending").exists():
                self._unlock(); return False
            state = self._request("/activity")
            if state.get("maintenance") is False:
                self.held = False
                self._unlock()
                return self.acquire()
            if state.get("maintenance") is not True:
                self._unlock(); return False
            self._write_marker()
            self.held = True
            deadline = time.monotonic() + self.timeout
            while state.get("quiescent") is not True:
                if time.monotonic() >= deadline:
                    # Keep admission closed; another attempt can recover it.
                    self.held = False; self._unlock(); return False
                time.sleep(self.interval)
                state = self._request("/activity")
                if state.get("maintenance") is not True:
                    raise AdmissionError("maintenance_unconfirmed")
            return True
        except Exception:
            self.held = False; self._unlock()
            raise AdmissionError("maintenance_unavailable") from None

    def acquire_offline_for_recovery(self):
        """Caller has stopped the server and holds the updater's switching record."""
        if self.lock is not None:
            return False
        lock = (self.directory / "lifecycle.lock").open("a+b")
        try:
            fcntl.flock(lock, fcntl.LOCK_EX | fcntl.LOCK_NB)
        except BlockingIOError:
            lock.close(); return False
        self.lock = lock
        if (self.directory / "idle-stop.pending").exists():
            self._unlock(); return False
        self._write_marker()
        self.held = True
        return True

    def release(self):
        if not self.held: raise AdmissionError("maintenance_not_owned")
        if self._request("/maintenance/release", "POST").get("maintenance") is not False:
            raise AdmissionError("release_unconfirmed")
        self._remove_marker()
        self.held = False
        self.closed_at = None
        self._unlock()

    def _unlock(self):
        if self.lock is not None:
            self.lock.close(); self.lock = None
