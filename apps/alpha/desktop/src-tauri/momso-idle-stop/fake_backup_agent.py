"""Test-only object backup agent for the real-process idle stop test.

The server runs it as `/usr/bin/python3 <agent> backup` with a cleared
environment, so it is configured only through files next to the server data
directory. It never touches the network.
"""
import os
from pathlib import Path
import sys
import time


def main():
    if sys.argv[1:] != ["backup"]:
        raise SystemExit(2)
    root = Path(os.environ["MOMSO_SERVER_DATA_DIR"]).parent
    runs = root / "fake-agent-runs"
    count = int(runs.read_text()) + 1 if runs.exists() else 1
    runs.write_text(str(count))
    delays = [float(value) for value in (root / "fake-agent-delays").read_text().split(",")]
    delay = delays[min(count, len(delays)) - 1]
    print("momso-backup-v3:snapshot-ready", flush=True)
    time.sleep(delay)
    print("momso-backup-v3:uploaded", flush=True)


if __name__ == "__main__":
    main()
