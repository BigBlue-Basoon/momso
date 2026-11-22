"""Linux service adapter; admission and gateway are provided by their owners."""
import json
import os
from pathlib import Path
import subprocess
import time
import urllib.request

from data_snapshots import DataSnapshots
from update import _atomic_json


class SystemdHost:
    def __init__(self, root, data_directory, unit, health_url, admission, gateway, progress):
        self.root = Path(root)
        self.current = self.root / "current"
        self.sequence = self.root / "installed.json"
        self.unit = unit
        self.health_url = health_url
        self.admission = admission
        self.gateway = gateway
        self.report = progress
        self.snapshots = DataSnapshots(data_directory, self.root / "data-snapshots")

    def progress(self, value): self.report(value)
    def current_sequence(self): return json.loads(self.sequence.read_text())["sequence"]
    def record_sequence(self, value): _atomic_json(self.sequence, {"sequence": value})
    def current_release(self): return str(self.current.resolve(strict=True))
    def current_gateway(self): return self.gateway.current()
    def acquire_maintenance(self): return self.admission.acquire()
    def release_maintenance(self): self.admission.release()
    def stop_server(self): self._systemctl("stop")
    def start_server(self): self._systemctl("start")
    def _systemctl(self, operation):
        subprocess.run(["systemctl", operation, self.unit], check=True, stdout=subprocess.DEVNULL, stderr=subprocess.DEVNULL, timeout=30)
    def snapshot_data(self): return self.snapshots.snapshot()
    def restore_data(self, snapshot): self.snapshots.restore(snapshot)
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
    def activate_gateway(self, image): self.gateway.activate(image)
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
