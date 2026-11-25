"""Test-only file-backed object store and synthetic key; never used in production."""
import hashlib
import importlib.util
import os
from pathlib import Path
import sys
import tempfile

spec = importlib.util.spec_from_file_location("fixture_backup", MOMSO_FAKE_BACKUP_MODULE)
backup = importlib.util.module_from_spec(spec)
spec.loader.exec_module(backup)
from cryptography.hazmat.primitives.ciphers.aead import AESGCM

root = Path(__file__).parent / "fake-object-store"
root.mkdir(exist_ok=True)


class Keys:
    def wrap(self, value):
        nonce = os.urandom(12)
        return nonce + AESGCM(bytes(32)).encrypt(nonce, value, b"fixture")

    def unwrap(self, value):
        return AESGCM(bytes(32)).decrypt(value[:12], value[12:], b"fixture")


class Store:
    def read_purge_ledger(self):
        path = root / "purged-originals"
        return path.read_bytes() if path.exists() else None

    def read_purge_ledger_versioned(self):
        value = self.read_purge_ledger()
        return (hashlib.sha256(value).hexdigest(), value) if value is not None else None

    def write_purge_ledger_if_generation(self, generation, content):
        current = self.read_purge_ledger_versioned()
        if generation != (current[0] if current else "0"):
            raise ValueError("generation changed")
        (root / "purged-originals").write_bytes(content)

    def modified(self, slot):
        path = root / ("slot-" + str(slot))
        return path.stat().st_mtime_ns if path.exists() else None

    def read(self, slot):
        return (root / ("slot-" + str(slot))).read_bytes()

    def write(self, slot, content):
        (root / ("slot-" + str(slot))).write_bytes(content)

    def read_versioned(self, slot):
        path = root / ("slot-" + str(slot))
        if not path.exists():
            return None
        content = path.read_bytes()
        return hashlib.sha256(content).hexdigest(), content

    def create_slot_if_absent(self, slot, content):
        with (root / ("slot-" + str(slot))).open("xb") as file:
            file.write(content)

    def replace_if_generation(self, slot, generation, content):
        current = self.read_versioned(slot)
        if current is None or current[0] != generation:
            raise ValueError("generation changed")
        self.write(slot, content)

    def delete_slot_if_generation(self, slot, generation):
        current = self.read_versioned(slot)
        if current is None or current[0] != generation:
            raise ValueError("generation changed")
        (root / ("slot-" + str(slot))).unlink()

    def write_chunk(self, identifier, index, content):
        with (root / (identifier + "-" + str(index))).open("xb") as file:
            file.write(content)

    def read_chunk(self, identifier, index):
        return (root / (identifier + "-" + str(index))).read_bytes()

    def chunk_matches(self, identifier, index, size, checksum):
        path = root / (identifier + "-" + str(index))
        if not path.exists():
            return False
        data = path.read_bytes()
        return len(data) == size and hashlib.md5(data, usedforsecurity=False).hexdigest() == checksum


data = Path(os.environ["MOMSO_SERVER_DATA_DIR"])
if sys.argv[1] in ("check-purge", "record-purge"):
    import json
    if sys.argv[1] == "record-purge" and (root / "block-purge").exists():
        raise RuntimeError("synthetic unavailable")
    digest = json.load(sys.stdin)["digest"]
    backup.plan_original_purge(Store(), Keys(), digest)
    if sys.argv[1] == "record-purge":
        backup.record_purged_original(Store(), Keys(), digest)
    print("momso-backup-v3:purge-recorded" if sys.argv[1] == "record-purge" else "momso-backup-v3:purge-checked")
elif sys.argv[1] == "fixture-mark-purged":
    import json
    digest = sys.argv[2]
    (root / "purged-originals").write_bytes(backup._encrypt(json.dumps({"version": 1, "digests": [digest]}).encode(), Keys()))
elif sys.argv[1] == "restore-assets":
    import json
    try:
        backup.restore_assets(data, data / "archive", Store(), Keys(), json.load(sys.stdin))
    except Exception as error:
        (root / "diagnostic").write_text(type(error).__name__ + ": " + str(error))
        raise
    print("momso-backup-v3:assets-restored")
elif sys.argv[1] == "backup":
    assert os.environ.get("MOMSO_BACKUP_QUIESCENT") == "1"
    assert "MOMSO_SERVER_TOKEN" not in os.environ
    identity = Path(os.environ["CREDENTIALS_DIRECTORY"], "momso-vault-identity").read_bytes()
    with tempfile.TemporaryDirectory(prefix="fixture-snapshot-") as directory:
        staged = Path(directory)
        private = backup.stage_live_snapshot(data, staged)
        if os.environ.get("MOMSO_BACKUP_SNAPSHOT_SIGNAL") == "1":
            print("momso-backup-v3:snapshot-ready", flush=True)
        result = backup.backup(staged, Store(), Keys(), backup.DATABASE_AND_VAULT,
                               archive=staged / "archive", quiescent=True, vault_identity=identity,
                               memory_files=private)
        print("momso-backup-v3:" + result)
else:
    result = backup.restore(data, Store(), Keys(), archive=data / "archive",
                            vault_identity_writer=lambda _: "created")
    assert result["status"] == "restored"
