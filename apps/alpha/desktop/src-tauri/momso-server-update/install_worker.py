"""Install an independent updater after verifying the running baseline."""
import json
import os
from pathlib import Path
import re
import subprocess
import tempfile

from layout_migration import prepare_verified_baseline


def _write(path, data, mode=0o644):
    path = Path(path)
    path.parent.mkdir(parents=True, exist_ok=True)
    fd, name = tempfile.mkstemp(prefix=".momso-install-", dir=path.parent)
    try:
        with os.fdopen(fd, "wb") as stream:
            stream.write(data)
            stream.flush()
            os.fsync(stream.fileno())
        os.chmod(name, mode)
        os.replace(name, path)
    finally:
        if os.path.exists(name):
            os.unlink(name)


def install(flat, unit_dir, worker_dir, config_path, config, updater, manifest, signature, *, run=subprocess.run):
    """Keep the old services running; return only after the worker is enabled."""
    flat, units, worker, config_path = map(Path, (flat, unit_dir, worker_dir, config_path))
    root = Path(config["updateRoot"])
    for value in (str(root), str(worker), str(config_path)):
        if not re.fullmatch(r"/[A-Za-z0-9_./-]+", value) or ".." in Path(value).parts:
            raise ValueError("invalid_configuration")
    proxy = units / "momso-private-proxy.service"
    server_override = units / "momso-server.service.d" / "momso-update.conf"
    service = units / "momso-update.service"
    marker = root / ".momso-update-active"
    original_proxy = proxy.read_bytes()
    old = original_proxy.decode("utf-8")
    old_exec = f"ExecStart=/usr/bin/python3 {flat}/momso_anywhere_vm_proxy.py"
    new_exec = f"ExecStart=/usr/bin/python3 {root}/current/momso_anywhere_vm_proxy.py"
    if old_exec not in old and new_exec not in old:
        raise ValueError("proxy_unit_unrecognized")
    new_proxy = old.replace("Requires=momso-server.service\n", "").replace(old_exec, new_exec).encode()
    override = ("[Service]\nExecStart=\nExecStart=" + str(root) + "/current/momso-server\n"
                "Environment=MOMSO_SERVER_WEB_DIR=" + str(root) + "/current/web\n").encode()
    unit = ("[Unit]\nDescription=momso update worker\nAfter=network-online.target momso-server.service\n"
            "Wants=network-online.target momso-server.service\n[Service]\nType=simple\n"
            f"ExecStart=/usr/bin/python3 {worker}/worker.py --config {config_path}\n"
            "Restart=on-failure\nRestartSec=60\n[Install]\nWantedBy=multi-user.target\n").encode()
    saved = {path: path.read_bytes() if path.exists() else None for path in (server_override, service, config_path, marker)}
    if saved[server_override] not in (None, override):
        raise ValueError("server_override_unrecognized")
    if saved[service] not in (None, unit):
        raise ValueError("worker_unit_unrecognized")
    if not updater.host.acquire_maintenance():
        raise ValueError("server_busy")
    try:
        prepare_verified_baseline(flat, updater, manifest, signature)
        _write(config_path, json.dumps(config).encode(), 0o600)
        _write(server_override, override)
        _write(proxy, new_proxy)
        _write(service, unit)
        run(["systemctl", "daemon-reload"], check=True, timeout=30)
        # Restart only the private proxy; the live server keeps running until update.
        run(["systemctl", "restart", "momso-private-proxy.service"], check=True, timeout=30)
        _write(marker, b"momso-update-v1\n", 0o600)
    except BaseException:
        _write(proxy, original_proxy)
        for path, previous in saved.items():
            if previous is None:
                path.unlink(missing_ok=True)
            else:
                _write(path, previous, 0o600 if path == config_path else 0o644)
        run(["systemctl", "daemon-reload"], check=False, timeout=30)
        run(["systemctl", "restart", "momso-private-proxy.service"], check=False, timeout=30)
        raise
    finally:
        updater.host.release_maintenance()
    run(["systemctl", "enable", "--now", "momso-update.service"], check=True, timeout=30)
