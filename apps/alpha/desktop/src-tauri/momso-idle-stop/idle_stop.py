"""Fail-closed idle stop for a single Compute Engine VM."""
import datetime as dt
import fcntl
import json
import os
from pathlib import Path
import re
import tempfile
import time
import urllib.error
import urllib.parse
import urllib.request


class IdleError(Exception):
    pass


def _request(url, *, headers=None, method="GET", body=None):
    request = urllib.request.Request(url, headers=headers or {}, method=method, data=body)
    # No proxy, redirects, raw response logging, or request token in subprocess args.
    opener = urllib.request.build_opener(urllib.request.ProxyHandler({}), NoRedirect())
    with opener.open(request, timeout=10) as response:
        payload = response.read(65537)
        if len(payload) > 65536:
            raise IdleError("response_too_large")
        return payload


class NoRedirect(urllib.request.HTTPRedirectHandler):
    def redirect_request(self, req, fp, code, msg, headers, newurl):
        return None


class Server:
    def __init__(self, token_file, port=8765):
        if not (1 <= int(port) <= 65535): raise ValueError("invalid_port")
        self.base = f"http://127.0.0.1:{int(port)}"
        self.token_file = Path(token_file)

    def call(self, path, method="GET"):
        token = self.token_file.read_text(encoding="utf-8").strip()
        if not token or "\n" in token or "\r" in token: raise IdleError("token_unavailable")
        body = b"" if method == "POST" else None
        raw = _request(self.base + path, method=method, body=body, headers={"Authorization": "Bearer " + token})
        result = json.loads(raw)
        if not isinstance(result, dict): raise IdleError("invalid_activity")
        return result


class Compute:
    METADATA = "http://metadata.google.internal/computeMetadata/v1/"
    API = "https://compute.googleapis.com/compute/v1"
    def stop_self(self):
        headers = {"Metadata-Flavor": "Google"}
        def metadata(path): return _request(self.METADATA + path, headers=headers).decode("utf-8").strip()
        project = metadata("project/project-id")
        zone = metadata("instance/zone").split("/")[-1]
        instance = metadata("instance/name")
        if any(not re.fullmatch(r"[a-z][a-z0-9-]{0,62}", value) for value in (project, zone, instance)):
            raise IdleError("metadata_invalid")
        token = json.loads(metadata("instance/service-accounts/default/token"))["access_token"]
        if not isinstance(token, str) or not token or "\n" in token or "\r" in token:
            raise IdleError("token_unavailable")
        url = f"{self.API}/projects/{project}/zones/{zone}/instances/{instance}/stop"
        operation = json.loads(_request(url, method="POST", body=b"", headers={"Authorization": "Bearer " + token}))
        if not isinstance(operation, dict) or not operation.get("name") or operation.get("error"):
            raise IdleError("stop_unconfirmed")
        return True


def _atomic_marker(path, boot_id):
    path.parent.mkdir(mode=0o700, parents=True, exist_ok=True)
    with tempfile.NamedTemporaryFile(mode="w", encoding="utf-8", dir=path.parent, delete=False) as file:
        tmp = Path(file.name)
        try:
            file.write(boot_id + "\n"); file.flush(); os.fsync(file.fileno()); os.replace(tmp, path)
        finally:
            tmp.unlink(missing_ok=True)
    fd = os.open(path.parent, os.O_RDONLY | os.O_DIRECTORY)
    try: os.fsync(fd)
    finally: os.close(fd)


def _remove_marker(path):
    path.unlink(missing_ok=True)
    fd = os.open(path.parent, os.O_RDONLY | os.O_DIRECTORY)
    try: os.fsync(fd)
    finally: os.close(fd)


def _idle(view, now, idle_seconds, *, gated):
    if (view.get("shutdownReady") is not True or view.get("busy") is not False
            or view.get("maintenance") is not gated or (gated and view.get("quiescent") is not True)):
        return False
    for key in ("activeTurns", "pendingApprovals", "pendingInputs", "activeWrites", "githubLogins", "admittedRequests"):
        value = view.get(key)
        if type(value) is not int or value != 0: return False
    if view.get("providerLogin") is not False: return False
    try:
        stamp = dt.datetime.fromisoformat(view["lastActivityAt"].replace("Z", "+00:00"))
        elapsed = (now - stamp).total_seconds()
    except (KeyError, TypeError, ValueError): return False
    return elapsed >= idle_seconds


def wait_for_recovery(monitor, poll_seconds):
    # An updater may hold the lifecycle lock while restarting this service.
    # Wait without touching the server or the recovery marker until it releases.
    while True:
        result = monitor.recover()
        if result != "lifecycle_busy":
            return result
        time.sleep(poll_seconds)


class IdleStop:
    def __init__(self, server, compute, state_dir, idle_seconds=1800, boot_id=None):
        if idle_seconds <= 0: raise ValueError("invalid_idle_seconds")
        self.server, self.compute = server, compute
        self.state = Path(state_dir); self.state.mkdir(mode=0o700, parents=True, exist_ok=True)
        self.marker = self.state / "idle-stop.pending"
        self.lock = self.state / "lifecycle.lock"
        self.idle_seconds = idle_seconds
        self.boot_id = boot_id or Path("/proc/sys/kernel/random/boot_id").read_text(encoding="ascii").strip()
        if not re.fullmatch(r"[0-9a-f-]{36}", self.boot_id): raise ValueError("invalid_boot_id")

    def _lock(self):
        handle = self.lock.open("a+b")
        try: fcntl.flock(handle, fcntl.LOCK_EX | fcntl.LOCK_NB)
        except BlockingIOError:
            handle.close(); return None
        return handle

    def recover(self):
        handle = self._lock()
        if handle is None: return "lifecycle_busy"
        with handle:
            if not self.marker.exists(): return "no_recovery"
            previous_boot = self.marker.read_text(encoding="ascii").strip()
            if not re.fullmatch(r"[0-9a-f-]{36}", previous_boot): raise IdleError("marker_invalid")
            if previous_boot == self.boot_id: return "stop_pending"
            response = self.server.call("/maintenance/release", "POST")
            if response.get("maintenance") is not False: raise IdleError("release_unconfirmed")
            _remove_marker(self.marker)
            return "resumed"

    def step(self, now=None):
        now = now or dt.datetime.now(dt.timezone.utc)
        handle = self._lock()
        if handle is None: return "lifecycle_busy"
        with handle:
            if self.marker.exists(): return "stop_pending"
            before = self.server.call("/activity")
            if not _idle(before, now, self.idle_seconds, gated=False): return "not_ready"
            _atomic_marker(self.marker, self.boot_id)
            try:
                acquired = self.server.call("/maintenance", "POST")
                if acquired.get("maintenance") is not True: raise IdleError("admission_unconfirmed")
                after = self.server.call("/activity")
                if not _idle(after, dt.datetime.now(dt.timezone.utc), self.idle_seconds, gated=True):
                    released = self.server.call("/maintenance/release", "POST")
                    if released.get("maintenance") is not False: raise IdleError("release_unconfirmed")
                    _remove_marker(self.marker)
                    return "activity_changed"
                self.compute.stop_self()
                return "stop_requested"
            except Exception:
                # A stop request may have reached Compute even if its response was lost.
                # Keep admission closed and the recovery marker until restart/operator check.
                raise IdleError("stop_or_gate_unconfirmed") from None


def main():
    import argparse
    parser = argparse.ArgumentParser()
    parser.add_argument("--token-file", required=True)
    parser.add_argument("--state-dir", required=True)
    parser.add_argument("--port", type=int, default=8765)
    parser.add_argument("--idle-seconds", type=int, default=1800)
    parser.add_argument("--poll-seconds", type=int, default=60)
    parser.add_argument("--recover-only", action="store_true")
    args = parser.parse_args()
    if args.poll_seconds <= 0: raise SystemExit("invalid_poll_seconds")
    monitor = IdleStop(Server(args.token_file, args.port), Compute(), args.state_dir, args.idle_seconds)
    try:
        recovered = wait_for_recovery(monitor, args.poll_seconds)
        if recovered == "stop_pending": raise IdleError("stop_pending")
        if args.recover_only: return
        while True:
            try: monitor.step()
            except Exception: print("idle_stop_unavailable", flush=True)
            time.sleep(args.poll_seconds)
    except Exception:
        raise SystemExit("idle_stop_recovery_required") from None


if __name__ == "__main__": main()
