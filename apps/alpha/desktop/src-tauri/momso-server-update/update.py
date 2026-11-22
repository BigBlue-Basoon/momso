"""Standalone Linux update coordinator. The embedding host supplies lifecycle hooks."""
from __future__ import annotations
import fcntl
import hashlib
import json
import os
from pathlib import Path, PurePosixPath
import shutil
import tarfile
import tempfile
import urllib.request
from urllib.parse import urlparse
from release_manifest import ManifestError, verify as verify_manifest


class UpdateError(Exception):
    pass


def _download(url, allow_http=False, limit=536870912):
    parsed = urlparse(url)
    if parsed.username or parsed.password or not parsed.hostname or parsed.scheme not in ({"https", "http"} if allow_http else {"https"}):
        raise UpdateError("invalid_url")
    with urllib.request.urlopen(url, timeout=30) as response:
        if not allow_http and urlparse(response.geturl()).scheme != "https":
            raise UpdateError("invalid_url")
        data = response.read(limit + 1)
    if len(data) > limit:
        raise UpdateError("download_too_large")
    return data


def _extract(bundle, output):
    import io
    with tarfile.open(fileobj=io.BytesIO(bundle), mode="r:gz") as archive:
        seen = set()
        total = 0
        members = archive.getmembers()
        if len(members) > 100000:
            raise UpdateError("invalid_bundle")
        for item in members:
            total += item.size
            if total > 2147483648:
                raise UpdateError("invalid_bundle")
            parts = PurePosixPath(item.name).parts
            if (not parts or item.name.startswith("/") or ".." in parts or item.name in seen
                    or not (item.isfile() or item.isdir())):
                raise UpdateError("invalid_bundle")
            seen.add(item.name)
        archive.extractall(output, **({'filter': 'data'} if hasattr(tarfile, 'data_filter') else {}))
    for name in ("momso-server", "momso_mobile_proxy.py", "momso_anywhere_vm_proxy.py", "web/index.html"):
        if not (output / name).is_file():
            raise UpdateError("invalid_bundle")
    # The bootstrap and PC installer use this marker to recognize owned web files.
    (output / "web" / ".momso-owned").write_text("momso", encoding="utf-8")
    (output / "momso-server").chmod(0o755)


def _atomic_json(path, value):
    path.parent.mkdir(mode=0o700, parents=True, exist_ok=True)
    with tempfile.NamedTemporaryFile(mode="w", encoding="utf-8", dir=path.parent, delete=False) as stream:
        tmp = Path(stream.name)
        try:
            json.dump(value, stream)
            stream.flush()
            os.fsync(stream.fileno())
            os.replace(tmp, path)
        finally:
            tmp.unlink(missing_ok=True)
    fd = os.open(path.parent, os.O_RDONLY | os.O_DIRECTORY)
    try:
        os.fsync(fd)
    finally:
        os.close(fd)


class Updater:
    """Host must hold an exclusive maintenance lease until release_maintenance()."""
    def __init__(self, root, public_key, host, *, allow_http_for_tests=False, channel='production'):
        self.root = Path(root)
        self.public_key = Path(public_key)
        self.host = host
        self.allow_http = allow_http_for_tests
        self.channel = channel
        self.root.mkdir(mode=0o755, parents=True, exist_ok=True)
        # The service runs as the unprivileged momso user and must traverse releases.
        # Recovery records are atomic mode-0600 files; data snapshots are mode 0700.
        os.chmod(self.root, 0o755)
        self.state = self.root / "update-state.json"

    def _emit(self, state, reason=None):
        # A disconnected screen must never prevent a rollback.
        try:
            self.host.progress({"state": state, "reason": reason})
        except Exception:
            pass

    def check(self, manifest_url, signature_url):
        manifest = _download(manifest_url, self.allow_http, 65536)
        signature = _download(signature_url, self.allow_http, 1024)
        try:
            return verify_manifest(manifest, signature, self.public_key, allow_http=self.allow_http, channel=self.channel)
        except ManifestError as error:
            raise UpdateError(str(error)) from None

    def install(self, manifest_url, signature_url):
        with (self.root / "update.lock").open("a+b") as lock:
            try:
                fcntl.flock(lock, fcntl.LOCK_EX | fcntl.LOCK_NB)
            except BlockingIOError:
                raise UpdateError("update_busy") from None
            try:
                return self._install(manifest_url, signature_url)
            except UpdateError as error:
                self._emit("failed", str(error))
                raise
            except (OSError, ValueError):
                self._emit("failed", "update_unavailable")
                raise UpdateError("update_unavailable") from None

    def _install(self, manifest_url, signature_url):
        if self.state.exists() and json.loads(self.state.read_text(encoding="utf-8")).get("phase") == "switching":
            raise UpdateError("recovery_required")
        self._emit("checking")
        release = self.check(manifest_url, signature_url)
        installed = self.host.current_sequence()
        if release["sequence"] <= installed:
            self._emit("up_to_date")
            return "up_to_date"
        self._emit("downloading")
        bundle = _download(release["bundleUrl"], self.allow_http)
        if hashlib.sha256(bundle).hexdigest() != release["bundleSha256"]:
            raise UpdateError("digest_mismatch")
        stage = Path(tempfile.mkdtemp(prefix="release-" + str(release["sequence"]) + "-", dir=self.root))
        try:
            _extract(bundle, stage)
            os.chmod(stage, 0o755)
        except tarfile.TarError:
            shutil.rmtree(stage)
            raise UpdateError("invalid_bundle") from None
        except Exception:
            shutil.rmtree(stage)
            raise
        self._emit("waiting_for_idle")
        if not self.host.acquire_maintenance():
            shutil.rmtree(stage)
            return "busy"
        try:
            old_release = self.host.current_release()
            old_gateway = self.host.current_gateway()
        except Exception:
            self.host.release_maintenance()
            raise UpdateError("current_state_unavailable") from None
        snapshot = None
        try:
            # Write the recovery record before stopping the old server.
            _atomic_json(self.state, {"phase": "switching", "oldRelease": old_release, "oldGateway": old_gateway, "snapshot": None, "newRelease": str(stage), "sequence": release["sequence"], "oldSequence": installed})
            # The lease must stop admission, wait for turns/writes/backups and remain held.
            self._emit("backing_up")
            self.host.stop_server()
            snapshot = self.host.snapshot_data()
            _atomic_json(self.state, {"phase": "switching", "oldRelease": old_release, "oldGateway": old_gateway, "snapshot": snapshot, "newRelease": str(stage), "sequence": release["sequence"], "oldSequence": installed})
            self._emit("installing")
            self.host.activate_release(str(stage))
            self.host.migrate_data(str(stage))
            self.host.start_server()
            if not self.host.server_healthy():
                raise UpdateError("server_unhealthy")
            self._emit("updating_gateway")
            self.host.activate_gateway(release["gatewayImage"])
            if not self.host.gateway_healthy():
                raise UpdateError("gateway_unhealthy")
            self.host.record_sequence(release["sequence"])
            _atomic_json(self.state, {"phase": "complete", "sequence": release["sequence"]})
            self._emit("completed")
            return "completed"
        except Exception:
            self._emit("rolling_back")
            try:
                self.host.stop_server()
                if snapshot is not None:
                    self.host.restore_data(snapshot)
                self.host.activate_release(old_release)
                self.host.activate_gateway(old_gateway)
                self.host.record_sequence(installed)
                self.host.start_server()
                if not self.host.server_healthy() or not self.host.gateway_healthy():
                    raise UpdateError("rollback_unhealthy")
                _atomic_json(self.state, {"phase": "rolled_back", "sequence": installed})
            except Exception as error:
                self._emit("recovery_required", "rollback_failed")
                # Never reopen admission after an uncertain recovery.
                raise UpdateError("rollback_failed") from error
            self._emit("rolled_back")
            raise UpdateError("update_failed_rolled_back") from None
        finally:
            if self.state.exists() and json.loads(self.state.read_text(encoding="utf-8")).get("phase") in {"complete", "rolled_back"}:
                self.host.release_maintenance()

    def recover(self):
        """Run before the host accepts requests after an interrupted update."""
        with (self.root / "update.lock").open("a+b") as lock:
            fcntl.flock(lock, fcntl.LOCK_EX | fcntl.LOCK_NB)
            if not self.state.exists():
                return "no_recovery"
            record = json.loads(self.state.read_text(encoding="utf-8"))
            if record.get("phase") != "switching":
                return "no_recovery"
            acquire = getattr(self.host, "acquire_recovery_maintenance", self.host.acquire_maintenance)
            if not acquire():
                raise UpdateError("busy")
            try:
                self._emit("rolling_back")
                self.host.stop_server()
                if record["snapshot"] is not None:
                    self.host.restore_data(record["snapshot"])
                self.host.activate_release(record["oldRelease"])
                self.host.activate_gateway(record["oldGateway"])
                self.host.record_sequence(record["oldSequence"])
                self.host.start_server()
                if not self.host.server_healthy() or not self.host.gateway_healthy():
                    raise UpdateError("rollback_unhealthy")
                _atomic_json(self.state, {"phase": "rolled_back", "sequence": record["oldSequence"]})
                self._emit("rolled_back")
                self.host.release_maintenance()
                return "rolled_back"
            except Exception as error:
                self._emit("recovery_required", "rollback_failed")
                raise UpdateError("rollback_failed") from error
