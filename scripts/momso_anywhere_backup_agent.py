"""VM entry point; requires separately provisioned bucket, KMS key and IAM."""
import argparse
import fcntl
import os
import json
import re
import sys
import tempfile
import shutil
from pathlib import Path

import momso_anywhere_backup as backup
from momso_anywhere_backup_google import CloudKms, CloudStorage, GoogleClient
from momso_anywhere_vault_identity import rewrap_for_host


def main():
    parser = argparse.ArgumentParser()
    parser.add_argument("action", choices=("backup", "restore", "restore-assets", "check-purge", "record-purge"))
    args = parser.parse_args()
    data = Path(os.environ.get("MOMSO_SERVER_DATA_DIR", "/var/lib/momso"))
    runtime = Path(os.environ.get("MOMSO_BACKUP_RUNTIME_DIR", data / ".backup-runtime"))
    if runtime.is_symlink():
        raise ValueError("unsafe backup runtime")
    runtime.mkdir(mode=0o700, parents=True, exist_ok=True)
    os.chmod(runtime, 0o700)
    client = GoogleClient()
    store = CloudStorage(client, os.environ["MOMSO_BACKUP_BUCKET"])
    keys = CloudKms(client, os.environ["MOMSO_BACKUP_KMS_KEY"])
    flags = os.O_RDWR | os.O_CREAT | getattr(os, "O_NOFOLLOW", 0) | getattr(os, "O_CLOEXEC", 0)
    with os.fdopen(os.open(runtime / "lock", flags, 0o600), "a+b") as lock:
        fcntl.flock(lock, fcntl.LOCK_EX)
        for stale in runtime.glob("snapshot-*"):
            if stale.is_symlink() or not stale.is_dir():
                raise ValueError("unsafe backup snapshot")
            shutil.rmtree(stale)
        if args.action in ("check-purge", "record-purge"):
            request = json.load(sys.stdin)
            if not isinstance(request, dict) or set(request) != {"digest"}:
                raise ValueError("invalid purge request")
            backup.plan_original_purge(store, keys, request["digest"])
            if args.action == "record-purge":
                backup.record_purged_original(store, keys, request["digest"])
            result = "purge-recorded" if args.action == "record-purge" else "purge-checked"
        elif args.action == "backup":
            if os.environ.get("MOMSO_BACKUP_QUIESCENT") != "1":
                raise ValueError("quiescence_required")
            restore_marker = data / '.momso-restore-pending-first-backup'
            if restore_marker.is_symlink():
                raise ValueError('unsafe_restore_marker')
            protect_slots = restore_marker.exists()
            protected_slot = None
            if protect_slots:
                if not restore_marker.is_file():
                    raise ValueError('unsafe_restore_marker')
                marker = restore_marker.read_bytes()
                if marker == b'momso-restore-v1\n':
                    report = data / '.momso-prestart-restore.json'
                    if report.is_symlink() or not report.is_file():
                        raise ValueError('restore_source_unknown')
                    source = json.loads(report.read_bytes()).get('restoredBackupId')
                else:
                    match = re.fullmatch(rb'momso-restore-v2:(slot-[0-7]@[0-9]{1,32})\n', marker)
                    source = match.group(1).decode('ascii') if match else None
                match = re.fullmatch(r'slot-([0-7])@([0-9]{1,32})', source or '')
                if not match:
                    raise ValueError('unsafe_restore_marker')
                protected_slot = int(match.group(1))
                current = store.read_versioned(protected_slot)
                if current is None or current[0] != match.group(2):
                    raise ValueError('restore_source_changed')
            credential_root = os.environ.get("CREDENTIALS_DIRECTORY")
            credential = Path(credential_root) / "momso-vault-identity" if credential_root else None
            identity = credential.read_bytes() if credential and credential.is_file() else None
            if os.stat(data).st_dev != os.stat(runtime).st_dev:
                raise ValueError("reservation_unavailable")
            amount = backup.estimate_snapshot_reservation(data)
            with backup.reserve_large_write(data, amount):
                with tempfile.TemporaryDirectory(prefix="snapshot-", dir=runtime) as directory:
                    staged = Path(directory)
                    private = backup.stage_live_snapshot(data, staged)
                    if os.environ.get("MOMSO_BACKUP_SNAPSHOT_SIGNAL") == "1":
                        print("momso-backup-v3:snapshot-ready", flush=True)
                    result = backup.backup(staged, store, keys, backup.DATABASE_AND_VAULT, temp_root=runtime,
                                           vault_identity=identity, archive=staged / "archive", quiescent=True,
                                           memory_files=private, preserve_existing_slots=protect_slots,
                                           protected_slot=protected_slot)
                    if protect_slots and result == 'uploaded':
                        restore_marker.unlink()
                        directory = os.open(data, os.O_RDONLY | getattr(os, 'O_DIRECTORY', 0))
                        try: os.fsync(directory)
                        finally: os.close(directory)
        elif args.action == "restore-assets":
            backup.restore_assets(data, data / "archive", store, keys, json.load(sys.stdin))
            result = "assets-restored"
        else:
            target = os.environ.get("MOMSO_BACKUP_VAULT_CREDENTIAL")
            writer = (lambda identity: rewrap_for_host(identity, target)) if target else None
            result = backup.restore(data, store, keys, vault_identity_writer=writer, archive=data / "archive")["status"]
        print("momso-backup-v3:" + result if args.action in ("backup", "restore-assets", "check-purge", "record-purge") else result)


if __name__ == "__main__":
    try:
        main()
    except ValueError as error:
        if str(error) in {"disk_low", "reservation_busy", "reservation_unavailable"}:
            print("momso-backup-v3:" + ("backup_disk_low" if str(error) == "disk_low" else str(error)), flush=True)
            sys.exit(1)
        raise
