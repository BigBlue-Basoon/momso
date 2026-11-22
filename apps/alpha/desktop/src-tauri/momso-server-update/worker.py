"""Separate service entry point; configuration is provisioned by the installer."""
import argparse
import json
from pathlib import Path
import signal
import threading

from cloud_run_gateway import CloudRunGateway
from github_releases import GitHubReleases, ReleasePoller
from progress_status import ProgressFile
from server_admission import ServerAdmission
from systemd_host import SystemdHost
from update import Updater


def make_poller(config):
    channel = config["channel"]
    root = Path(config["updateRoot"])
    progress = ProgressFile(config["statusFile"])
    admission = ServerAdmission(config["serverTokenFile"], config["lifecycleDir"])
    gateway = CloudRunGateway(config["project"], config["region"], config["gatewayService"])
    host = SystemdHost(root, config["dataDir"], config["serverUnit"],
                       "http://127.0.0.1:8765/health", admission, gateway, progress,
                       proxy_unit=config["proxyUnit"], idle_unit=config.get("idleUnit", "momso-idle-stop.service"))
    updater = Updater(root, config["publicKey"], host, channel=channel)
    return ReleasePoller(GitHubReleases(channel), updater)


def main():
    parser = argparse.ArgumentParser()
    parser.add_argument("--config", required=True)
    args = parser.parse_args()
    try:
        config = json.loads(Path(args.config).read_text(encoding="utf-8"))
        poller = make_poller(config)
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
