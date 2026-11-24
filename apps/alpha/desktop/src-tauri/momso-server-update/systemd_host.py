"""Linux service adapter; admission and gateway are provided by their owners."""
import json
import os
from pathlib import Path
import subprocess
import time
import urllib.request
import sys

from data_snapshots import DataSnapshots
from server_admission import AdmissionError
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
    def restore_data(self, snapshot): self.snapshots.restore(snapshot)
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
        names = ('momso_mobile_proxy.py', 'momso_anywhere_vm_proxy.py')
        if any((Path(old_release) / name).read_bytes() != (Path(new_release) / name).read_bytes() for name in names):
            subprocess.run(["systemctl", "restart", self.proxy_unit], check=True, stdout=subprocess.DEVNULL, stderr=subprocess.DEVNULL, timeout=30)
    def restart_worker(self):
        os.execv(sys.executable, [sys.executable, str(self.helpers.worker / 'worker.py'), *sys.argv[1:]])
    def activate_gateway(self, image): self.gateway.activate(image)
    def preflight_gateway(self, image):
        if hasattr(self.gateway, "preflight"): return self.gateway.preflight(image)
    def gateway_healthy(self): return self.gateway.healthy()
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
