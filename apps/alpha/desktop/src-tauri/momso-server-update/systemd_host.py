"""Linux service adapter; admission and gateway are provided by their owners."""
import json
import datetime as dt
import hashlib
import hmac
import os
import re
import shutil
import ssl
from pathlib import Path
import subprocess
import time
import urllib.request
import urllib.parse
import sys

from data_snapshots import DataSnapshots, reserve_large_write, estimate_snapshot_bytes
from server_admission import AdmissionError
from momso_anywhere_proxy_files import PROXY_FILES
from update import _atomic_json
from managed_helpers import ManagedHelpers


class SystemdHost:
    def __init__(self, root, data_directory, unit, health_url, admission, gateway, progress, *, proxy_unit=None, idle_unit=None, helpers=None):
        self.root = Path(root)
        self.current = self.root / "current"
        self.sequence = self.root / "installed.json"
        self.unit = unit
        self.proxy_unit = proxy_unit
        self.idle_unit = idle_unit
        self.health_url = health_url
        self.admission = admission
        self.gateway = gateway
        self.report = progress
        self.snapshots = DataSnapshots(data_directory, self.root / "data-snapshots")
        self.helpers = helpers or ManagedHelpers()
        self.worker_restart_required = False

    def progress(self, value): self.report(value)
    def current_sequence(self): return json.loads(self.sequence.read_text())["sequence"]
    def record_sequence(self, value): _atomic_json(self.sequence, {"sequence": value})
    def current_release(self): return str(self.current.resolve(strict=True))
    def current_gateway(self): return self.gateway.current()
    def acquire_maintenance(self): return self.admission.acquire()
    def admission_pending(self):
        pending = getattr(self.admission, "pending", None)
        return bool(pending and pending())
    def acquire_recovery_maintenance(self):
        if hasattr(self.admission, "reacquire_for_recovery"):
            try:
                return self.admission.reacquire_for_recovery()
            except AdmissionError:
                # A switching journal exists, but the new server may not start.
                # Stop restart attempts before taking the lifecycle lock offline.
                try:
                    self.stop_server()
                except Exception:
                    self._restart_after_failed_acquire()
                    raise
                try:
                    acquired = self.admission.acquire_offline_for_recovery()
                except Exception:
                    self._restart_after_failed_acquire()
                    raise
                if not acquired:
                    self._restart_after_failed_acquire()
                return acquired
        return self.admission.acquire()
    def _restart_after_failed_acquire(self):
        try:
            self.start_server()
        finally:
            self.server_healthy()
    def release_maintenance(self): self.admission.release()
    def stop_server(self): self._systemctl("stop")
    def start_server(self):
        failed = False
        for unit in (self.unit, self.proxy_unit, self.idle_unit):
            if not unit: continue
            try:
                subprocess.run(["systemctl", "start", unit], check=True, stdout=subprocess.DEVNULL, stderr=subprocess.DEVNULL, timeout=30)
            except (OSError, subprocess.SubprocessError):
                if unit == self.idle_unit:
                    try:
                        result = subprocess.run(["systemctl", "show", unit, "--property=LoadState", "--value"], capture_output=True, check=False, timeout=30)
                        if result.returncode == 0 and result.stdout.strip() == b"not-found": continue
                    except (OSError, subprocess.SubprocessError):
                        pass
                failed = True
        if failed: raise RuntimeError("service_start_failed")
    def _systemctl(self, operation):
        subprocess.run(["systemctl", operation, self.unit], check=True, stdout=subprocess.DEVNULL, stderr=subprocess.DEVNULL, timeout=30)
    def snapshot_data(self): return self.snapshots.snapshot()

    def reserve_update(self, release_bytes):
        # Snapshot and release storage must be on the reservation filesystem.
        if os.stat(self.root).st_dev != os.stat(self.snapshots.data).st_dev:
            raise ValueError('reservation_unavailable')
        needed = release_bytes + 2 * estimate_snapshot_bytes(self.snapshots.data)
        return reserve_large_write(self.snapshots.data, needed)
    def restore_data(self, snapshot): self.snapshots.restore(snapshot)
    def cleanup_old_updates(self, record):
        """Remove only updater-owned old versions after successful release."""
        if record.get("phase") != "complete" or record.get("needsRelease") is not False:
            return
        count = os.environ.get("MOMSO_UPDATE_SNAPSHOT_KEEP", "2")
        if not re.fullmatch(r"[0-9]{1,3}", count) or int(count) < 1:
            raise ValueError("invalid_snapshot_retention")
        root = self.root.resolve(strict=True)
        current = self.current.resolve(strict=True)
        old = Path(record["oldRelease"]).resolve(strict=True)
        new = Path(record["newRelease"]).resolve(strict=True)
        if current != new or any(path.parent != root for path in (old, new)):
            raise ValueError("invalid_cleanup_state")
        for path in root.iterdir():
            if (path.is_symlink() or not path.is_dir() or path in (old, current)
                    or not re.fullmatch(r"release-[0-9]+-[a-z0-9_]{8}", path.name)):
                continue
            marker = path / "web" / ".momso-owned"
            if marker.is_symlink() or not marker.is_file() or marker.read_text(encoding="utf-8") != "momso":
                continue
            shutil.rmtree(path)
        snapshots = self.snapshots.snapshots
        protected = Path(record["snapshot"]).resolve(strict=True)
        if protected.parent != snapshots:
            raise ValueError("invalid_snapshot")
        candidates = [p for p in snapshots.iterdir() if not p.is_symlink() and p.is_dir()
                      and re.fullmatch(r"data-[a-z0-9_]{8}", p.name)
                      and (p / "data").is_dir() and not (p / "data").is_symlink()]
        retained = {p for p in sorted(candidates, key=lambda p: p.stat().st_mtime_ns, reverse=True)[:int(count)]}
        retained.add(protected)
        for path in candidates:
            if path not in retained:
                shutil.rmtree(path)
        report = root / "cleanup-status.json"
        _atomic_json(report, {"lastCleanupAt": dt.datetime.now(dt.timezone.utc).isoformat()})
        os.chmod(report, 0o644)
    def recovery_data_identity(self):
        try:
            path = self.snapshots.data
            if path.is_symlink() or not path.is_dir(): return None
            info = path.stat()
            return (info.st_dev, info.st_ino)
        except OSError:
            return None
    def activate_release(self, release):
        target = Path(release).resolve(strict=True)
        if self.root.resolve() not in target.parents:
            raise ValueError("invalid_release")
        temp = self.root / ".current-next"
        temp.unlink(missing_ok=True)
        temp.symlink_to(target, target_is_directory=True)
        os.replace(temp, self.current)
    def migrate_data(self, release):
        # The new server migrates its own DB on startup. Host admission stays closed.
        pass
    def apply_managed(self, release):
        self.worker_restart_required = self.helpers.apply(release)
        if self.helpers.changed_units:
            subprocess.run(["systemctl", "daemon-reload"], check=True, stdout=subprocess.DEVNULL, stderr=subprocess.DEVNULL, timeout=30)
    def rollback_managed(self):
        changed_units = (os.path.lexists(self.helpers.private / 'managed-s108-units-journal.json') or
                         os.path.lexists(self.helpers.private / 'managed-s108-overrides-journal.json'))
        changed_worker = os.path.lexists(self.helpers.pointer_journal) or os.path.lexists(self.helpers._groups()[2][2])
        if os.path.lexists(self.helpers.pointer_journal) or any(os.path.lexists(journal) for _, _, journal in self.helpers._groups()):
            self.helpers.rollback()
        if changed_units:
            subprocess.run(["systemctl", "daemon-reload"], check=True, stdout=subprocess.DEVNULL, stderr=subprocess.DEVNULL, timeout=30)
        self.worker_restart_required = changed_worker
    def commit_managed(self):
        if os.path.lexists(self.helpers.pointer_journal) or any(os.path.lexists(journal) for _, _, journal in self.helpers._groups()):
            self.helpers.commit()
    def refresh_proxy(self, old_release, new_release):
        if not self.proxy_unit or not old_release or not new_release: return
        if any(not (Path(old_release) / name).is_file() or (Path(old_release) / name).read_bytes() != (Path(new_release) / name).read_bytes() for name in PROXY_FILES):
            subprocess.run(["systemctl", "restart", self.proxy_unit], check=True, stdout=subprocess.DEVNULL, stderr=subprocess.DEVNULL, timeout=30)
    def restart_worker(self):
        os.execv(sys.executable, [sys.executable, str(self.helpers.worker / 'worker.py'), *sys.argv[1:]])
    def activate_gateway(self, image): self.gateway.activate(image)
    def preflight_gateway(self, image):
        if hasattr(self.gateway, "preflight"): return self.gateway.preflight(image)
    def gateway_healthy(self): return self.gateway.healthy()
    def gateway_proxy_healthy(self):
        try:
            url = self.gateway._describe().get('status', {}).get('url', '')
            parsed = urllib.parse.urlsplit(url)
            if parsed.scheme != 'https' or not parsed.hostname or not parsed.hostname.endswith('.run.app') or parsed.netloc != parsed.hostname or parsed.path or parsed.query or parsed.fragment:
                return False
            token = Path('/etc/momso-private/transport-token').read_bytes().strip()
            if not token: return False
            proof = hmac.new(token, b'momso-proxy-health-v1', hashlib.sha256).hexdigest()
            request = urllib.request.Request(url + '/__momso/proxy-health', headers={'X-Momso-Health-Proof': proof})
            class NoRedirect(urllib.request.HTTPRedirectHandler):
                def redirect_request(self, *args, **kwargs): return None
            with urllib.request.build_opener(NoRedirect()).open(request, timeout=30) as response:
                return response.status == 200 and json.load(response).get('state') == 'ready'
        except (OSError, ValueError, TypeError, AttributeError, KeyError):
            return False
    def proxy_healthy(self):
        if not self.proxy_unit:
            return False
        for _ in range(30):
            try:
                active = subprocess.run(['systemctl', 'is-active', '--quiet', self.proxy_unit], timeout=5).returncode == 0
                token = Path('/etc/momso-private/transport-token').read_text(encoding='utf-8').strip()
                request = urllib.request.Request('https://127.0.0.1:8766/health', headers={'Authorization': 'Bearer ' + token})
                context = ssl.create_default_context(cafile='/etc/momso-private/certificate.pem')
                context.check_hostname = False
                if active:
                    with urllib.request.urlopen(request, context=context, timeout=3) as response:
                        if response.status == 200 and json.load(response).get('state') == 'ready':
                            return True
            except (OSError, ValueError, TypeError, AttributeError):
                pass
            time.sleep(1)
        return False
    def server_healthy(self):
        for _ in range(40):
            try:
                if hasattr(self.admission, "health"):
                    if self.admission.health(): return True
                else:
                    with urllib.request.urlopen(self.health_url, timeout=1) as response:
                        if response.status == 200 and json.load(response).get("state") == "ready":
                            return True
            except Exception:
                pass
            time.sleep(0.25)
        return False
