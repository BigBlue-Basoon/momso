"""VM entry point; requires separately provisioned bucket, KMS key and IAM."""
import argparse
import fcntl
import os
import json
import sys
from pathlib import Path

import momso_anywhere_backup as backup
from momso_anywhere_backup_google import CloudKms, CloudStorage, GoogleClient
from momso_anywhere_vault_identity import rewrap_for_host


def main():
    parser = argparse.ArgumentParser()
    parser.add_argument("action", choices=("backup", "restore", "restore-assets"))
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
        if args.action == "backup":
            if os.environ.get("MOMSO_BACKUP_QUIESCENT") != "1":
                raise ValueError("quiescence_required")
            credential_root = os.environ.get("CREDENTIALS_DIRECTORY")
            credential = Path(credential_root) / "momso-vault-identity" if credential_root else None
            identity = credential.read_bytes() if credential and credential.is_file() else None
            result = backup.backup(data, store, keys, backup.DATABASE_AND_VAULT, temp_root=runtime,
                                   vault_identity=identity, archive=data / "archive", quiescent=True)
        elif args.action == "restore-assets":
            backup.restore_assets(data, data / "archive", store, keys, json.load(sys.stdin))
            result = "assets-restored"
        else:
            target = os.environ.get("MOMSO_BACKUP_VAULT_CREDENTIAL")
            writer = (lambda identity: rewrap_for_host(identity, target)) if target else None
            result = backup.restore(data, store, keys, vault_identity_writer=writer, archive=data / "archive")["status"]
        print("momso-backup-v3:" + result if args.action in ("backup", "restore-assets") else result)


if __name__ == "__main__":
    main()
