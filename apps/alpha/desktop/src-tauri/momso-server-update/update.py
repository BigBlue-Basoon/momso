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
from cloud_run_gateway import GatewayError


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
        # Only fixed state and reason codes are written to journald.
        import re
        if re.fullmatch(r"[a-z_]{1,80}", state) and (reason is None or re.fullmatch(r"[a-z_]{1,80}", reason)):
            print(f"update stage={state} reason={reason or 'none'}", flush=True)
        # A disconnected screen must never prevent a rollback.
        try:
            self.host.progress({"state": state, "reason": reason})
        except Exception:
            pass

    def _rollback(self, record):
        errors = []
        def attempt(stage, action):
            try:
                action()
            except Exception:
                errors.append(stage)
                self._emit("rolling_back", stage)
        attempt("server_stop_failed", self.host.stop_server)
        if record.get("snapshot") is not None:
            attempt("data_restore_failed", lambda: self.host.restore_data(record["snapshot"]))
        attempt("release_restore_failed", lambda: self.host.activate_release(record["oldRelease"]))
        if "release_restore_failed" not in errors:
            attempt("sequence_restore_failed", lambda: self.host.record_sequence(record["oldSequence"]))
        # Start all local services even if any earlier recovery step failed.
        attempt("server_start_failed", self.host.start_server)
        try:
            if not self.host.server_healthy():
                errors.append("server_unhealthy")
                self._emit("rolling_back", "server_unhealthy")
        except Exception:
            errors.append("server_health_failed")
            self._emit("rolling_back", "server_health_failed")
        try:
            try:
                unchanged = self.host.current_gateway() == record["oldGateway"]
            except Exception:
                unchanged = False
            if not unchanged: self.host.activate_gateway(record["oldGateway"])
        except Exception:
            errors.append("gateway_restore_failed")
            self._emit("rolling_back", "gateway_restore_failed")
        try:
            if not self.host.gateway_healthy():
                errors.append("gateway_unhealthy")
                self._emit("rolling_back", "gateway_unhealthy")
        except Exception:
            errors.append("gateway_health_failed")
            self._emit("rolling_back", "gateway_health_failed")
        if errors:
            raise UpdateError("rollback_failed")
        _atomic_json(self.state, {"phase": "rolled_back", "sequence": record["oldSequence"], "needsRelease": True})

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
        try:
            if hasattr(self.host, "preflight_gateway"):
                self.host.preflight_gateway(release["gatewayImage"])
        except GatewayError as error:
            raise UpdateError(str(error)) from None
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
        try:
            acquired = self.host.acquire_maintenance()
        except Exception:
            shutil.rmtree(stage)
            raise UpdateError("maintenance_unavailable") from None
        if not acquired:
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
            _atomic_json(self.state, {"phase": "complete", "sequence": release["sequence"], "needsRelease": True})
            self._emit("completed")
            return "completed"
        except Exception as failure:
            reason = str(failure) if isinstance(failure, (GatewayError, UpdateError)) and str(failure) in {"gateway_image_permission", "gateway_operation_failed", "gateway_unhealthy", "server_unhealthy"} else "update_failed"
            self._emit("rolling_back", reason)
            try:
                self._rollback({"snapshot": snapshot, "oldRelease": old_release, "oldGateway": old_gateway, "oldSequence": installed})
            except Exception as error:
                self._emit("recovery_required", "rollback_failed")
                # Never reopen admission after an uncertain recovery.
                raise UpdateError("rollback_failed") from error
            self._emit("rolled_back")
            raise UpdateError(reason if reason != "update_failed" else "update_failed_rolled_back") from None
        finally:
            if self.state.exists():
                record = json.loads(self.state.read_text(encoding="utf-8"))
                if record.get("phase") in {"complete", "rolled_back"} and record.get("needsRelease") is True:
                    try:
                        self.host.release_maintenance()
                    except Exception:
                        self._emit("failed", "maintenance_release_failed")
                        raise UpdateError("maintenance_release_failed") from None
                    record["needsRelease"] = False
                    _atomic_json(self.state, record)

    def recover(self):
        """Run before the host accepts requests after an interrupted update."""
        with (self.root / "update.lock").open("a+b") as lock:
            fcntl.flock(lock, fcntl.LOCK_EX | fcntl.LOCK_NB)
            if not self.state.exists():
                return "no_recovery"
            try:
                record = json.loads(self.state.read_text(encoding="utf-8"))
                if not isinstance(record, dict): raise ValueError("invalid_record")
            except (OSError, ValueError):
                try:
                    self.host.start_server()
                except Exception:
                    self._emit("recovery_required", "server_start_failed")
                finally:
                    try: self.host.server_healthy()
                    except Exception: pass
                    self._emit("recovery_required", "recovery_record_invalid")
                raise UpdateError("recovery_record_invalid") from None
            phase = record.get("phase")
            pending_release = phase in {"complete", "rolled_back"} and record.get("needsRelease") is True
            if phase != "switching" and not pending_release:
                return "no_recovery"
            acquire = getattr(self.host, "acquire_recovery_maintenance", self.host.acquire_maintenance)
            try:
                acquired = acquire()
            except Exception:
                self._emit("recovery_required", "recovery_lease_unavailable")
                raise UpdateError("recovery_lease_unavailable") from None
            if not acquired:
                raise UpdateError("busy")
            if pending_release:
                try:
                    self.host.release_maintenance()
                    record["needsRelease"] = False
                    _atomic_json(self.state, record)
                    return "released"
                except Exception:
                    self._emit("recovery_required", "maintenance_release_failed")
                    raise UpdateError("maintenance_release_failed") from None
            try:
                self._emit("rolling_back")
                self._rollback(record)
            except Exception as error:
                self._emit("recovery_required", "rollback_failed")
                raise UpdateError("rollback_failed") from error
            self._emit("rolled_back")
            try:
                self.host.release_maintenance()
                record = json.loads(self.state.read_text(encoding="utf-8"))
                record["needsRelease"] = False
                _atomic_json(self.state, record)
                return "rolled_back"
            except Exception:
                self._emit("recovery_required", "maintenance_release_failed")
                raise UpdateError("maintenance_release_failed") from None
