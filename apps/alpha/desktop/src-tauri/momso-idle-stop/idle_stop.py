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


def _http_reason(error):
    """HTTP status and the server's fixed error code; never the raw body."""
    code = f"HTTP{int(error.code)}" if 100 <= int(error.code) <= 599 else "HTTPError"
    try:
        reason = json.loads(error.read(4097)[:4096]).get("error")
    except Exception:
        reason = None
    return code + ":" + reason if isinstance(reason, str) and re.fullmatch(r"[a-z][a-z_]{0,63}", reason) else code


class IdleAttemptError(IdleError):
    def __init__(self, stage, error):
        # Never include exception text: HTTP errors can contain credentials.
        self.stage = stage
        if isinstance(error, urllib.error.HTTPError):
            self.kind = _http_reason(error)
        else:
            self.kind = type(error).__name__ if type(error).__module__ in ("builtins", "urllib.error", "socket", "json.decoder") else "IdleError"
        super().__init__(f"{self.stage}:{self.kind}")


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


USER_WORK = ("activeTurns", "pendingApprovals", "pendingInputs", "activeWrites", "githubLogins", "admittedRequests")


def _settling(view, before):
    """True when the gate is closed and no person's work appeared after it.

    POST /maintenance on the real server starts a fresh backup (it bumps the
    backup generation), so right after the gate closes /activity reports a
    running backup and shutdownReady false. That is the server settling, not
    new activity, and must not be treated as a reason to reopen admission.
    """
    if view.get("maintenance") is not True or view.get("providerLogin") is not False:
        return False
    for key in USER_WORK:
        value = view.get(key)
        if type(value) is not int or value != 0: return False
    purges = view.get("pendingPurges", 0)
    if type(purges) is not int or purges != 0: return False
    return isinstance(view.get("lastActivityAt"), str) and view.get("lastActivityAt") == before.get("lastActivityAt")


def wait_for_recovery(monitor, poll_seconds):
    # An updater may hold the lifecycle lock while restarting this service.
    # Wait without touching the server or the recovery marker until it releases.
    while True:
        result = monitor.recover()
        if result not in ("lifecycle_busy", "stop_pending"):
            return result
        time.sleep(poll_seconds)


class IdleStop:
    def __init__(self, server, compute, state_dir, idle_seconds=1800, boot_id=None, settle_seconds=300):
        if idle_seconds <= 0: raise ValueError("invalid_idle_seconds")
        if settle_seconds <= 0: raise ValueError("invalid_settle_seconds")
        self.settle_seconds, self.settle_poll = settle_seconds, 2
        self.clock, self.sleep = time.monotonic, time.sleep
        self.server, self.compute = server, compute
        self.state = Path(state_dir); self.state.mkdir(mode=0o700, parents=True, exist_ok=True)
        self.marker = self.state / "idle-stop.pending"
        self.lock = self.state / "lifecycle.lock"
        self.status = self.state / "idle-stop-status.json"
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
            if previous_boot == self.boot_id and self._recent_stop(): return "stop_pending"
            response = self.server.call("/maintenance/release", "POST")
            if response.get("maintenance") is not False: raise IdleError("release_unconfirmed")
            _remove_marker(self.marker)
            return "resumed"

    def _recent_stop(self):
        # A response may be lost after Compute accepted a stop. Allow that
        # operation time to finish before reopening admission on the same boot.
        try:
            status = json.loads(self.status.read_text(encoding="utf-8"))
            result = status["result"]
            timestamp = dt.datetime.fromisoformat(status["attemptedAt"])
            age = (dt.datetime.now(dt.timezone.utc) - timestamp).total_seconds()
            return (result == "stop_requested" or result.startswith("error:compute_stop:")) and 0 <= age < 300
        except (OSError, ValueError, KeyError, TypeError, AttributeError):
            return False

    def report(self, result):
        payload = json.dumps({"attemptedAt": dt.datetime.now(dt.timezone.utc).isoformat(), "result": result})
        with tempfile.NamedTemporaryFile(mode="w", encoding="utf-8", dir=self.state, delete=False) as file:
            tmp = Path(file.name)
            try:
                file.write(payload); file.flush(); os.fsync(file.fileno()); os.replace(tmp, self.status)
            finally:
                tmp.unlink(missing_ok=True)

    def _release(self):
        """Reopen admission, waiting out a backup the gate itself started.

        The server refuses release with 409 while a backup or admitted request
        is running. Retry those for the settle window; anything else, or a 409
        that outlasts the window, is raised and the marker stays so the next
        iteration reopens (S162).
        """
        deadline = self.clock() + self.settle_seconds
        while True:
            try:
                released = self.server.call("/maintenance/release", "POST")
            except urllib.error.HTTPError as exc:
                if exc.code != 409 or self.clock() >= deadline: raise
                exc.close()
                self.sleep(self.settle_poll)
                continue
            if released.get("maintenance") is not False: raise IdleError("release_unconfirmed")
            return

    def step(self, now=None):
        now = now or dt.datetime.now(dt.timezone.utc)
        handle = self._lock()
        if handle is None: return "lifecycle_busy"
        with handle:
            if self.marker.exists():
                previous_boot = self.marker.read_text(encoding="ascii").strip()
                if not re.fullmatch(r"[0-9a-f-]{36}", previous_boot): raise IdleError("marker_invalid")
                if previous_boot == self.boot_id and self._recent_stop(): return "stop_pending"
                try:
                    self._release()
                except Exception as exc:
                    raise IdleAttemptError("reopen", exc) from None
                _remove_marker(self.marker)
                return "resumed"
            before = self.server.call("/activity")
            if not _idle(before, now, self.idle_seconds, gated=False): return "not_ready"
            _atomic_marker(self.marker, self.boot_id)
            stage = "maintenance"
            try:
                acquired = self.server.call("/maintenance", "POST")
                if acquired.get("maintenance") is not True: raise IdleError("admission_unconfirmed")
                stage = "activity"
                deadline = self.clock() + self.settle_seconds
                while True:
                    after = self.server.call("/activity")
                    if _idle(after, dt.datetime.now(dt.timezone.utc), self.idle_seconds, gated=True):
                        break
                    settling = _settling(after, before)
                    if not settling or self.clock() >= deadline:
                        stage = "release"
                        self._release()
                        _remove_marker(self.marker)
                        return "settle_timeout" if settling else "activity_changed"
                    self.sleep(self.settle_poll)
                stage = "compute_stop"
                # Persist the possible in-flight stop before sending it; a lost
                # response or service crash must not reopen admission immediately.
                self.report("stop_requested")
                self.compute.stop_self()
                return "stop_requested"
            except Exception as exc:
                raise IdleAttemptError(stage, exc) from None


def configured_idle_seconds(argument, environ):
    """An explicit command-line value wins over the optional service setting."""
    if argument is not None:
        return argument
    value = environ.get("MOMSO_IDLE_SECONDS")
    if value is None:
        return 1800
    if not re.fullmatch(r"[0-9]+", value) or int(value) <= 0:
        raise ValueError("invalid_idle_seconds")
    return int(value)


def main():
    import argparse
    parser = argparse.ArgumentParser()
    parser.add_argument("--token-file", required=True)
    parser.add_argument("--state-dir", required=True)
    parser.add_argument("--port", type=int, default=8765)
    parser.add_argument("--idle-seconds", type=int)
    parser.add_argument("--poll-seconds", type=int, default=60)
    parser.add_argument("--settle-seconds", type=int, default=300)
    parser.add_argument("--recover-only", action="store_true")
    args = parser.parse_args()
    if args.poll_seconds <= 0: raise SystemExit("invalid_poll_seconds")
    if args.settle_seconds <= 0: raise SystemExit("invalid_settle_seconds")
    try:
        idle_seconds = configured_idle_seconds(args.idle_seconds, os.environ)
    except ValueError:
        raise SystemExit("invalid_idle_seconds") from None
    monitor = IdleStop(Server(args.token_file, args.port), Compute(), args.state_dir, idle_seconds,
                       settle_seconds=args.settle_seconds)
    delay = args.poll_seconds
    while True:
        try:
            wait_for_recovery(monitor, args.poll_seconds)
            break
        except Exception as exc:
            reason = "recovery:" + (type(exc).__name__ if type(exc).__module__ == "builtins" else "IdleError")
            print("idle_stop_unavailable reason=" + reason, flush=True)
            try: monitor.report("error:" + reason)
            except OSError: pass
            delay = min(max(args.poll_seconds, delay * 2), 900)
            time.sleep(delay)
    if args.recover_only: return
    delay = args.poll_seconds
    while True:
        try:
            result = monitor.step()
            if result not in ("not_ready", "lifecycle_busy", "stop_pending"):
                print("idle_stop_result result=" + result, flush=True)
                monitor.report(result)
            delay = args.poll_seconds
        except Exception as exc:
            reason = str(exc) if isinstance(exc, IdleAttemptError) else "recovery:" + (type(exc).__name__ if type(exc).__module__ == "builtins" else "IdleError")
            print("idle_stop_unavailable reason=" + reason, flush=True)
            try: monitor.report("error:" + reason)
            except OSError: pass
            delay = min(max(args.poll_seconds, delay * 2), 900)
        time.sleep(delay)


if __name__ == "__main__": main()
