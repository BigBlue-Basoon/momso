"""Separate service entry point; configuration is provisioned by the installer."""
import argparse
import base64
import json
import os
from pathlib import Path
import signal
import subprocess
import threading

from cloud_run_gateway import CloudRunGateway
from github_releases import GitHubReleases, ReleasePoller
from progress_status import ProgressFile
from server_admission import ServerAdmission
from systemd_host import SystemdHost
from update import Updater
from trust_rotation import RootStore


JOURNAL_LIMIT = b"# Managed by momso\n[Journal]\nSystemMaxUse=200M\nRuntimeMaxUse=64M\n"


def ensure_journal_limit(directory=Path("/etc/systemd/journald.conf.d"), *, run=subprocess.run):
    """Install a dedicated drop-in, preserving any pre-existing different file."""
    directory = Path(directory)
    if directory.is_symlink(): return "conflict"
    directory.mkdir(mode=0o755, parents=True, exist_ok=True)
    path = directory / "90-momso.conf"
    if os.path.lexists(path):
        if path.is_symlink() or not path.is_file() or path.read_bytes() != JOURNAL_LIMIT:
            return "conflict"
    else:
        flags = os.O_WRONLY | os.O_CREAT | os.O_EXCL | getattr(os, "O_NOFOLLOW", 0)
        fd = os.open(path, flags, 0o644)
        with os.fdopen(fd, "wb") as file:
            file.write(JOURNAL_LIMIT); file.flush(); os.fsync(file.fileno())
        fd = os.open(directory, os.O_RDONLY | os.O_DIRECTORY)
        try: os.fsync(fd)
        finally: os.close(fd)
    run(["systemctl", "try-restart", "systemd-journald.service"], check=True, timeout=30)
    return "applied"


def make_poller(config):
    channel = config["channel"]
    root = Path(config["updateRoot"])
    progress = ProgressFile(config["statusFile"])
    admission = ServerAdmission(config["serverTokenFile"], config["lifecycleDir"])
    gateway = CloudRunGateway(config["project"], config["region"], config["gatewayService"])
    host = SystemdHost(root, config["dataDir"], config["serverUnit"],
                       "http://127.0.0.1:8765/health", admission, gateway, progress,
                       proxy_unit=config["proxyUnit"], idle_unit=config.get("idleUnit", "momso-idle-stop.service"))
    trust_store = None
    if channel == 'production':
        trust_store = RootStore(config['trustState'], base64.b64decode(config['trustBootstrap'], validate=True))
    updater = Updater(root, config["publicKey"], host, channel=channel, trust_store=trust_store)
    return ReleasePoller(GitHubReleases(channel), updater)


def main():
    parser = argparse.ArgumentParser()
    parser.add_argument("--config", required=True)
    args = parser.parse_args()
    try:
        config = json.loads(Path(args.config).read_text(encoding="utf-8"))
        poller = make_poller(config)
        try:
            if ensure_journal_limit() != "applied": print("journal_limit_conflict", flush=True)
        except (OSError, subprocess.SubprocessError):
            print("journal_limit_unavailable", flush=True)
        stop = threading.Event()
        for sig in (signal.SIGINT, signal.SIGTERM):
            signal.signal(sig, lambda *_: stop.set())
        poller.run_forever(stop, interval_seconds=config.get("intervalSeconds", 3600))
        return 0
    except (OSError, ValueError, KeyError, TypeError):
        # Do not print configuration, credentials, paths or subprocess errors.
        print("update_worker_unavailable")
        return 1


if __name__ == "__main__":
    raise SystemExit(main())
