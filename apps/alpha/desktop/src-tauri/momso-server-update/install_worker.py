"""Install an independent updater after verifying the running baseline."""
import json
import os
from pathlib import Path
import re
import shutil
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


def install(flat, unit_dir, worker_dir, config_path, config, updater, manifest, signature, *, run=subprocess.run, gate_already_held=False, health_check=None):
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
    current = root / 'current'
    sequence = root / 'installed.json'
    baseline_absent = not os.path.lexists(current) and not os.path.lexists(sequence)
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
    if gate_already_held:
        backup = flat / '.momso-upgrade-backup'
        if (backup.is_symlink() or not backup.is_dir() or (backup / 'owner').is_symlink()
                or (backup / 'mode').is_symlink() or (backup / 'owner').read_text() != 'momso-upgrade-v1'
                or (backup / 'mode').read_text() != 'gated'):
            raise ValueError('maintenance_unverified')
        activity = updater.host.admission._request('/activity')
        if activity.get('maintenance') is not True or activity.get('quiescent') is not True:
            raise ValueError('maintenance_unverified')
    elif not updater.host.acquire_maintenance():
        raise ValueError("server_busy")
    enable_attempted = False
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
        if health_check is not None:
            health_check()
        enable_attempted = True
        run(["systemctl", "enable", "--now", "momso-update.service"], check=True, timeout=30)
    except BaseException:
        cleanup_failed = False
        if saved[service] is None and enable_attempted:
            try:
                run(["systemctl", "disable", "--now", "momso-update.service"], check=True, timeout=30)
            except Exception:
                cleanup_failed = True
        _write(proxy, original_proxy)
        for path, previous in saved.items():
            if previous is None:
                path.unlink(missing_ok=True)
            else:
                _write(path, previous, 0o600 if path == config_path else 0o644)
        run(["systemctl", "daemon-reload"], check=False, timeout=30)
        run(["systemctl", "restart", "momso-private-proxy.service"], check=False, timeout=30)
        if baseline_absent and current.is_symlink() and sequence.is_file() and not sequence.is_symlink():
            record = json.loads(sequence.read_text(encoding='utf-8'))
            name = record.get('baseline')
            target = current.resolve(strict=True)
            if (isinstance(name, str) and re.fullmatch(r'baseline-[A-Za-z0-9_-]+', name)
                    and target.parent == root.resolve() and target.name == name and target.is_dir()):
                current.unlink()
                sequence.unlink()
                shutil.rmtree(target)
        if cleanup_failed:
            raise RuntimeError('worker_stop_failed')
        raise
    finally:
        if not gate_already_held:
            updater.host.release_maintenance()
