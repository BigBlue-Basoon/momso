"""Encrypted, fixed-slot backup primitives for a customer-owned VM.

The storage and key interfaces deliberately contain no cloud credentials.  The
installer must supply implementations and schedule this module before it can
be used as a production backup.
"""
import hashlib
import datetime
import errno
import io
import json
import os
import re
import sqlite3
import shutil
import stat
import tarfile
import tempfile
import time
import uuid
from contextlib import closing, contextmanager
from pathlib import Path, PurePosixPath

from cryptography.hazmat.primitives.ciphers.aead import AESGCM
from cryptography.exceptions import InvalidTag

SLOT_COUNT = 7
FORMAT = b"momso-anywhere-backup-v1"
CHUNK_BYTES = 4 * 1024 * 1024
DATABASE_AND_VAULT = (
    "server.sqlite3", "class-student-private.sqlite3", "vault.age",
    "attachments/attachments.sqlite3", "saved-logins/vault.age",
    "provider-credentials/claude/vault.age", "provider-credentials/gemini/vault.age",
    "codex/auth.json", "github-repository.json", "chat-export-journal.json",
)


@contextmanager
def reserve_large_write(data_dir, requested_bytes):
    """Share the server's nonblocking, crash-released large-write reservation."""
    import fcntl
    root = Path(data_dir)
    if type(requested_bytes) is not int or requested_bytes < 0 or requested_bytes > 2**64 - 1:
        raise ValueError("reservation_unavailable")
    owner = root.stat().st_uid
    files = []
    try:
        for name in (".momso-reservation-state.lock", ".momso-reservation-operation.lock"):
            try:
                fd = os.open(root / name, os.O_RDWR | getattr(os, "O_NOFOLLOW", 0) | getattr(os, "O_CLOEXEC", 0))
            except OSError:
                raise ValueError("reservation_unavailable") from None
            file = os.fdopen(fd, "r+b", buffering=0); files.append(file)
            metadata = os.fstat(fd)
            if not stat.S_ISREG(metadata.st_mode) or metadata.st_nlink != 1 or metadata.st_uid != owner or metadata.st_mode & 0o077:
                raise ValueError("reservation_unavailable")
        state, operation = files
        try:
            fcntl.flock(state.fileno(), fcntl.LOCK_EX | fcntl.LOCK_NB)
            fcntl.flock(operation.fileno(), fcntl.LOCK_EX | fcntl.LOCK_NB)
        except BlockingIOError:
            raise ValueError("reservation_busy") from None
        if shutil.disk_usage(root).free < 1536 * 1024 * 1024 + requested_bytes:
            raise ValueError("disk_low")
        operation.seek(0); operation.truncate(0)
        value = (str(requested_bytes) + "\n").encode("ascii")
        if operation.write(value) != len(value): raise ValueError("reservation_unavailable")
        os.fsync(operation.fileno())
        fcntl.flock(state.fileno(), fcntl.LOCK_UN)
        yield
    finally:
        for file in reversed(files): file.close()


def _relative(name):
    if not isinstance(name, str) or not name or "\\" in name or any(ord(c) < 32 for c in name):
        raise ValueError("invalid backup filename")
    value = PurePosixPath(name)
    if value.is_absolute() or any(part in ("", ".", "..") for part in name.split("/")):
        raise ValueError("invalid backup filename")
    return value.parts


def _safe_path(root, name, create_parents=False):
    parts = _relative(name)
    current = root
    for part in parts[:-1]:
        current = current / part
        if current.is_symlink():
            raise ValueError("backup parent is a symlink")
        if create_parents:
            current.mkdir(mode=0o700, exist_ok=True)
        if current.exists() and not current.is_dir():
            raise ValueError("backup parent is not a directory")
    result = current / parts[-1]
    if result.is_symlink():
        raise ValueError("backup source is a symlink")
    return result


def _snapshot(data_dir, temp_dir, names):
    """Copy a small explicit allowlist; never traverse the momso archive."""
    result = {}
    for name in sorted(set(names)):
        source = _safe_path(data_dir, name)
        if not source.exists():
            continue
        if name.endswith((".sqlite", ".sqlite3", ".db")):
            target = temp_dir / (hashlib.sha256(name.encode("utf-8")).hexdigest() + ".sqlite3")
            with closing(sqlite3.connect(f"{source.as_uri()}?mode=ro", uri=True)) as src:
                with closing(sqlite3.connect(target)) as dst:
                    src.backup(dst)
            result[name] = target.read_bytes()
        else:
            # Saved-login vault updates are atomic renames; an open handle sees
            # either the old complete file or the new complete file.
            result[name] = source.read_bytes()
    return result


def stage_live_snapshot(data_dir, target):
    """Freeze the local backup inputs before releasing server admission."""
    data_dir, target = Path(data_dir), Path(target)
    memory_files = {}
    target.mkdir(mode=0o700, parents=True, exist_ok=True)
    for name in DATABASE_AND_VAULT:
        source = _safe_path(data_dir, name)
        if not source.exists():
            continue
        if name == "codex/auth.json":
            memory_files[name] = source.read_bytes()
            continue
        destination = _safe_path(target, name, create_parents=True)
        if name.endswith((".sqlite", ".sqlite3", ".db")):
            with closing(sqlite3.connect(f"{source.as_uri()}?mode=ro", uri=True)) as src:
                with closing(sqlite3.connect(destination)) as dst:
                    src.backup(dst)
        else:
            shutil.copyfile(source, destination, follow_symlinks=False)
    for name in ("archive/objects/sha256", "attachments/upload-staging"):
        source = _safe_path(data_dir, name)
        if not source.exists():
            continue
        if source.is_symlink() or not source.is_dir():
            raise ValueError("unsafe backup input")
        destination = _safe_path(target, name, create_parents=True)
        destination.mkdir(mode=0o700, exist_ok=True)
        for root, directories, files in os.walk(source, followlinks=False):
            root = Path(root)
            for item in directories:
                if (root / item).is_symlink():
                    raise ValueError("unsafe backup input")
                (destination / (root / item).relative_to(source)).mkdir(mode=0o700, exist_ok=True)
            for item in files:
                src = root / item
                if src.is_symlink() or not src.is_file():
                    raise ValueError("unsafe backup input")
                dst = destination / src.relative_to(source)
                if name.startswith("archive/"):
                    try:
                        os.link(src, dst, follow_symlinks=False)
                    except OSError as error:
                        if error.errno != errno.EXDEV:
                            raise
                        shutil.copyfile(src, dst, follow_symlinks=False)
                else:
                    shutil.copyfile(src, dst, follow_symlinks=False)
    return memory_files


def estimate_snapshot_reservation(data_dir):
    """Bound space for frozen inputs and a verified local restore of those inputs."""
    root = Path(data_dir)
    total = 0
    for name in DATABASE_AND_VAULT:
        path = _safe_path(root, name)
        if path.exists():
            total += path.stat().st_size
        if name.endswith((".sqlite", ".sqlite3", ".db")):
            wal = _safe_path(root, name + "-wal")
            if wal.exists():
                total += wal.stat().st_size
    for name in ("archive/objects/sha256", "attachments/upload-staging"):
        source = _safe_path(root, name)
        if not source.exists():
            continue
        for directory, directories, files in os.walk(source, followlinks=False):
            for entry in directories + files:
                path = Path(directory) / entry
                if path.is_symlink():
                    raise ValueError("unsafe backup input")
                if path.is_file():
                    total += path.stat().st_size
    return total * 3 + 64 * 1024 * 1024


def _pack(files):
    out = io.BytesIO()
    with tarfile.open(fileobj=out, mode="w") as tar:
        for name, content in sorted(files.items()):
            entry = tarfile.TarInfo(name)
            entry.size = len(content)
            entry.mode = 0o600
            tar.addfile(entry, io.BytesIO(content))
    return out.getvalue()


def _unpack(payload):
    result = {}
    with tarfile.open(fileobj=io.BytesIO(payload), mode="r:") as tar:
        for member in tar:
            if not member.isfile() or member.name in result:
                raise ValueError("invalid backup entry")
            _relative(member.name)
            source = tar.extractfile(member)
            result[member.name] = source.read()
    return result


def _encrypt(payload, key_service):
    key = AESGCM.generate_key(bit_length=256)
    nonce = os.urandom(12)
    return json.dumps({
        "version": 1,
        "wrapped_key": key_service.wrap(key).hex(),
        "nonce": nonce.hex(),
        "ciphertext": AESGCM(key).encrypt(nonce, payload, FORMAT).hex(),
    }, separators=(",", ":")).encode("ascii")


def _decrypt(blob, key_service):
    envelope = json.loads(blob)
    if envelope.get("version") != 1:
        raise ValueError("unknown backup format")
    key = key_service.unwrap(bytes.fromhex(envelope["wrapped_key"]))
    return AESGCM(key).decrypt(bytes.fromhex(envelope["nonce"]), bytes.fromhex(envelope["ciphertext"]), FORMAT)


def _object_descriptor(entry):
    if not isinstance(entry, dict) or set(entry) != {"sha256", "size", "id", "wrappedKey", "chunks", "partMd5"}:
        raise ValueError("invalid object descriptor")
    digest, size, count, identifier, hashes = entry["sha256"], entry["size"], entry["chunks"], entry["id"], entry["partMd5"]
    if (not isinstance(digest, str) or not re.fullmatch(r"[0-9a-f]{64}", digest)
            or not isinstance(identifier, str) or not re.fullmatch(r"[0-9a-f]{32}", identifier)
            or type(size) is not int or size < 0 or type(count) is not int
            or count != (size + CHUNK_BYTES - 1) // CHUNK_BYTES
            or not isinstance(hashes, list) or len(hashes) != count
            or any(not isinstance(value, str) or not re.fullmatch(r"[0-9a-f]{32}", value) for value in hashes)
            or not isinstance(entry["wrappedKey"], str) or not re.fullmatch(r"[0-9a-f]+", entry["wrappedKey"])):
        raise ValueError("invalid object descriptor")
    return digest, size, count, identifier


def _hash_file(path):
    flags = os.O_RDONLY | getattr(os, "O_NOFOLLOW", 0) | getattr(os, "O_CLOEXEC", 0)
    fd = os.open(path, flags)
    try:
        if not stat.S_ISREG(os.fstat(fd).st_mode):
            raise ValueError("backup source is not a file")
        hasher = hashlib.sha256()
        with os.fdopen(fd, "rb", closefd=False) as source:
            while chunk := source.read(CHUNK_BYTES):
                hasher.update(chunk)
        return hasher.hexdigest()
    finally:
        os.close(fd)


def _backup_path(path, expected_hash, store, key_service, old=None):
    flags = os.O_RDONLY | getattr(os, "O_NOFOLLOW", 0) | getattr(os, "O_CLOEXEC", 0)
    descriptor = os.dup(path) if isinstance(path, int) else os.open(path, flags)
    try:
        if not stat.S_ISREG(os.fstat(descriptor).st_mode):
            raise ValueError("backup source is not a file")
        if old is not None:
            _, old_size, old_count, old_id = _object_descriptor(old)
            if len(key_service.unwrap(bytes.fromhex(old["wrappedKey"]))) != 32:
                raise ValueError("invalid object key")
            hasher = hashlib.sha256()
            size = 0
            with os.fdopen(descriptor, "rb", closefd=False) as source:
                while chunk := source.read(CHUNK_BYTES):
                    hasher.update(chunk)
                    size += len(chunk)
            if hasher.hexdigest() != expected_hash:
                raise ValueError("object integrity error")
            if size == old_size and all(store.chunk_matches(old_id, index,
                    min(CHUNK_BYTES, old_size - index * CHUNK_BYTES) + 28, old["partMd5"][index])
                    for index in range(old_count)):
                return old
            os.lseek(descriptor, 0, os.SEEK_SET)
        identifier = uuid.uuid4().hex
        key = AESGCM.generate_key(bit_length=256)
        wrapped = key_service.wrap(key)
        hasher = hashlib.sha256()
        size = 0
        count = 0
        part_hashes = []
        with os.fdopen(descriptor, "rb", closefd=False) as source:
            while chunk := source.read(CHUNK_BYTES):
                hasher.update(chunk)
                size += len(chunk)
                nonce = os.urandom(12)
                aad = FORMAT + b"/object/" + bytes.fromhex(expected_hash) + count.to_bytes(8, "big")
                encrypted = nonce + AESGCM(key).encrypt(nonce, chunk, aad)
                store.write_chunk(identifier, count, encrypted)
                part_hashes.append(hashlib.md5(encrypted, usedforsecurity=False).hexdigest())
                count += 1
        if hasher.hexdigest() != expected_hash:
            raise ValueError("object integrity error")
        return {"sha256": expected_hash, "size": size, "id": identifier,
                "wrappedKey": wrapped.hex(), "chunks": count, "partMd5": part_hashes}
    finally:
        os.close(descriptor)


def backup_objects(archive, store, key_service, previous=(), purged=()):
    """Encrypt immutable source objects in bounded chunks; never modify originals."""
    root = Path(archive) / "objects" / "sha256"
    for path in (Path(archive) / "objects", root):
        if path.is_symlink():
            raise ValueError("object directory is a symlink")
    if not root.exists():
        return []
    prior = {}
    for entry in previous:
        digest, _, _, _ = _object_descriptor(entry)
        if digest in prior:
            raise ValueError("duplicate object descriptor")
        prior[digest] = entry
    descriptors = []
    for shard in sorted(root.iterdir()):
        if shard.is_symlink() or not shard.is_dir() or not re.fullmatch(r"[0-9a-f]{2}", shard.name):
            raise ValueError("invalid object directory")
        for path in sorted(shard.iterdir()):
            if path.is_symlink() or not re.fullmatch(r"[0-9a-f]{64}", path.name) or not path.name.startswith(shard.name):
                raise ValueError("invalid object path")
            if path.name in purged:
                raise ValueError("purged original remains in archive")
            descriptors.append(_backup_path(path, path.name, store, key_service, prior.get(path.name)))
    return descriptors


def restore_objects(archive, store, key_service, descriptors):
    """Verify all bytes before installing an absent object; never replace a file."""
    root = Path(archive) / "objects" / "sha256"
    for path in (Path(archive), Path(archive) / "objects", root):
        if path.is_symlink():
            raise ValueError("object directory is a symlink")
        path.mkdir(mode=0o700, exist_ok=True)
    for entry in descriptors:
        digest, size, count, identifier = _object_descriptor(entry)
        shard = root / digest[:2]
        if shard.is_symlink():
            raise ValueError("object directory is a symlink")
        shard.mkdir(mode=0o700, exist_ok=True)
        target = shard / digest
        if target.is_symlink():
            raise ValueError("object destination is a symlink")
        key = key_service.unwrap(bytes.fromhex(entry["wrappedKey"]))
        fd, temporary = tempfile.mkstemp(prefix=".momso-object-", dir=shard)
        try:
            hasher = hashlib.sha256()
            written = 0
            with os.fdopen(fd, "wb") as out:
                for index in range(count):
                    encrypted = store.read_chunk(identifier, index)
                    if hashlib.md5(encrypted, usedforsecurity=False).hexdigest() != entry["partMd5"][index]:
                        raise ValueError("encrypted object integrity error")
                    aad = FORMAT + b"/object/" + bytes.fromhex(digest) + index.to_bytes(8, "big")
                    chunk = AESGCM(key).decrypt(encrypted[:12], encrypted[12:], aad)
                    if len(chunk) != min(CHUNK_BYTES, size - written):
                        raise ValueError("object size mismatch")
                    out.write(chunk)
                    hasher.update(chunk)
                    written += len(chunk)
                out.flush()
                os.fsync(out.fileno())
            if written != size or hasher.hexdigest() != digest:
                raise ValueError("object integrity error")
            try:
                os.link(temporary, target)
            except FileExistsError:
                flags = os.O_RDONLY | getattr(os, "O_NOFOLLOW", 0) | getattr(os, "O_CLOEXEC", 0)
                existing = os.open(target, flags)
                try:
                    if not stat.S_ISREG(os.fstat(existing).st_mode):
                        raise ValueError("object destination is not a file")
                    with os.fdopen(existing, "rb", closefd=False) as source:
                        actual = hashlib.file_digest(source, "sha256").hexdigest()
                    if actual != digest:
                        raise ValueError("object destination conflict")
                finally:
                    os.close(existing)
        finally:
            os.unlink(temporary)


def verify_backup_blob(blob, store, key_service, names, temp_root=None):
    """Verify an encrypted snapshot by restoring it into a disposable directory.

    The caller must compare the storage generation again before pruning anything.
    This does not change the live data directory or the remote backup.
    """
    class SnapshotStore:
        def modified(self, slot):
            return 1 if slot == 0 else None

        def read(self, slot):
            if slot != 0:
                raise ValueError("invalid verification slot")
            return blob

        def read_purge_ledger(self):
            return store.read_purge_ledger()

        def read_chunk(self, identifier, index):
            return store.read_chunk(identifier, index)

    with tempfile.TemporaryDirectory(prefix=".momso-verify-", dir=temp_root) as directory:
        root = Path(directory)
        result = restore(root / "data", SnapshotStore(), key_service, names, slot=0,
                         vault_identity_writer=lambda _identity: "created",
                         archive=root / "archive")
        if result["status"] == "no_backup":
            raise ValueError("backup_changed")
        if result["status"] != "restored" or result["conflicts"]:
            raise ValueError("backup_verification_failed")
        files = _unpack(_decrypt(blob, key_service))
        ledger = read_purged_ledger(store, key_service)
        if ledger:
            files = without_originals(files, ledger)
        if ".momso-v3.json" in files:
            descriptors = json.loads(files[".momso-v3.json"])["descriptors"]
        else:
            descriptors = json.loads(files.get(".momso-objects.json", b"[]"))
        if not isinstance(descriptors, list):
            raise ValueError("invalid object manifest")
        restore_objects(root / "all-objects", store, key_service, descriptors)


def _snapshot_paths(data_dir, temporary, names):
    result = {}
    for name in sorted(set(names)):
        source = _safe_path(data_dir, name)
        if not source.exists():
            continue
        if name.endswith((".sqlite", ".sqlite3", ".db")):
            target = temporary / (hashlib.sha256(name.encode("utf-8")).hexdigest() + ".sqlite3")
            with closing(sqlite3.connect(f"{source.as_uri()}?mode=ro", uri=True)) as src:
                with closing(sqlite3.connect(target)) as dst:
                    src.backup(dst)
            result[name] = target
        else:
            result[name] = source
    return result


def _backup_v3(data_dir, archive, store, key_service, names, vault_identity, memory_files=None, preserve_existing_slots=False, protected_slot=None, temp_root=None):
    purged = read_purged_ledger(store, key_service)
    latest = _latest(store)
    previous_payload = _decrypt(store.read(latest[1]), key_service) if latest else None
    old_files = _unpack(previous_payload) if previous_payload else {}
    old_manifest = json.loads(old_files.get(".momso-v3.json", b"{}"))
    previous = old_manifest.get("descriptors", []) if old_manifest else json.loads(old_files.get(".momso-objects.json", b"[]"))
    if not isinstance(previous, list):
        raise ValueError("invalid object manifest")
    previous_by_hash = {_object_descriptor(entry)[0]: entry for entry in previous}
    if len(previous_by_hash) != len(previous):
        raise ValueError("duplicate object descriptor")
    with tempfile.TemporaryDirectory(prefix=".momso-db-snapshot-", dir=data_dir) as tmp:
        paths = _snapshot_paths(data_dir, Path(tmp), names)
        has_vault = any(name == "vault.age" or name.endswith("/vault.age") for name in paths)
        if has_vault and not vault_identity:
            raise ValueError("vault_identity_unavailable")
        originals = backup_objects(archive, store, key_service, previous, purged)
        by_hash = {entry["sha256"]: entry for entry in originals}
        files = {}
        staging = {}

        def add_path(path):
            digest = _hash_file(path)
            existing = by_hash.get(digest) or previous_by_hash.get(digest)
            entry = _backup_path(path, digest, store, key_service, existing)
            by_hash[digest] = entry
            return digest

        for name, path in paths.items():
            files[name] = add_path(path)
        for name, content in (memory_files or {}).items():
            if name != "codex/auth.json" or not isinstance(content, bytes):
                raise ValueError("invalid in-memory backup input")
            digest = hashlib.sha256(content).hexdigest()
            descriptor = os.memfd_create("momso-backup-private", os.MFD_CLOEXEC)
            try:
                with os.fdopen(os.dup(descriptor), "wb") as output:
                    output.write(content)
                os.lseek(descriptor, 0, os.SEEK_SET)
                existing = by_hash.get(digest) or previous_by_hash.get(digest)
                by_hash[digest] = _backup_path(descriptor, digest, store, key_service, existing)
                files[name] = digest
            finally:
                os.close(descriptor)
        stage = _safe_path(data_dir, "attachments/upload-staging")
        if stage.exists():
            if not stage.is_dir():
                raise ValueError("invalid staging directory")
            for path in sorted(stage.iterdir()):
                if path.is_symlink() or not path.is_file():
                    raise ValueError("invalid staging file")
                try:
                    if str(uuid.UUID(path.name)) != path.name:
                        raise ValueError("invalid staging id")
                except (ValueError, AttributeError):
                    raise ValueError("invalid staging id") from None
                staging[path.name] = add_path(path)
        manifest = {"version": 3, "files": files, "staging": staging,
                    "originals": [entry["sha256"] for entry in originals],
                    "descriptors": [by_hash[key] for key in sorted(by_hash)]}
        packed = {".momso-v3.json": json.dumps(manifest, sort_keys=True, separators=(",", ":")).encode("utf-8")}
        if has_vault:
            packed[".momso-vault-identity"] = vault_identity
        payload = _pack(packed)
        if previous_payload and hashlib.sha256(previous_payload).digest() == hashlib.sha256(payload).digest():
            return "unchanged"
        _stage_and_rotate(store, key_service, names, _encrypt(payload, key_service),
                          preserve_existing_slots, protected_slot, temp_root)
        return "uploaded"


def _install_verified(source, target, digest, preserve_conflict):
    def install(candidate):
        try:
            os.link(source, candidate)
        except OSError as exc:
            if exc.errno != errno.EXDEV:
                raise
            fd, temporary = tempfile.mkstemp(prefix=".momso-copy-", dir=candidate.parent)
            try:
                with os.fdopen(fd, "wb") as out, open(source, "rb") as src:
                    shutil.copyfileobj(src, out, CHUNK_BYTES)
                    out.flush()
                    os.fsync(out.fileno())
                if _hash_file(temporary) != digest:
                    raise ValueError("restore integrity error")
                os.link(temporary, candidate)
            finally:
                os.unlink(temporary)
        if hasattr(os, "O_DIRECTORY"):
            fd = os.open(candidate.parent, os.O_RDONLY | os.O_DIRECTORY)
            try:
                os.fsync(fd)
            finally:
                os.close(fd)
    if target.is_symlink():
        raise ValueError("restore destination is a symlink")
    if target.exists():
        if _hash_file(target) == digest:
            return None
        if not preserve_conflict:
            raise ValueError("restore destination conflict")
        number = 0
        while True:
            suffix = ".recovered" if number == 0 else f".recovered-{number}"
            candidate = Path(str(target) + suffix)
            try:
                install(candidate)
                return candidate.name
            except FileExistsError:
                if not candidate.is_symlink() and _hash_file(candidate) == digest:
                    return candidate.name
                number += 1
    try:
        install(target)
    except FileExistsError:
        return _install_verified(source, target, digest, preserve_conflict)
    return None


def _restore_v3(data_dir, archive, store, key_service, manifest, names, identity, vault_identity_writer, purged=()):
    if (not isinstance(manifest, dict) or set(manifest) != {"version", "files", "staging", "originals", "descriptors"}
            or manifest["version"] != 3 or not isinstance(manifest["files"], dict)
            or not isinstance(manifest["staging"], dict) or not isinstance(manifest["originals"], list)
            or not isinstance(manifest["descriptors"], list)):
        raise ValueError("invalid backup manifest")
    if not set(manifest["files"]).issubset(set(names)):
        raise ValueError("unexpected backup entry")
    descriptors = {}
    for entry in manifest["descriptors"]:
        digest, _, _, _ = _object_descriptor(entry)
        if digest in descriptors:
            raise ValueError("duplicate object descriptor")
        descriptors[digest] = entry
    for name, digest in manifest["files"].items():
        _relative(name)
        if digest not in descriptors:
            raise ValueError("missing backup descriptor")
    for name, digest in manifest["staging"].items():
        try:
            if str(uuid.UUID(name)) != name:
                raise ValueError("invalid staging id")
        except (ValueError, AttributeError):
            raise ValueError("invalid staging id") from None
        if digest not in descriptors:
            raise ValueError("missing backup descriptor")
    if any(value not in descriptors for value in manifest["originals"]):
        raise ValueError("missing backup descriptor")
    if any(value in manifest["originals"] or value in manifest["files"].values()
           or value in manifest["staging"].values() for value in purged):
        raise ValueError("purged backup contains original")
    if any(name == "vault.age" or name.endswith("/vault.age") for name in manifest["files"]):
        if not identity:
            raise ValueError("vault_identity_unavailable")
        if vault_identity_writer is None:
            raise ValueError("vault_identity_writer_unavailable")
    if data_dir.is_symlink() or Path(archive).is_symlink():
        raise ValueError("restore destination is a symlink")
    data_dir.mkdir(mode=0o700, parents=True, exist_ok=True)
    Path(archive).mkdir(mode=0o700, parents=True, exist_ok=True)
    conflicts = []
    with tempfile.TemporaryDirectory(prefix=".momso-restore-v3-", dir=data_dir) as tmp:
        virtual = Path(tmp) / "archive"
        restore_objects(virtual, store, key_service, list(descriptors.values()))
        def source(digest):
            return virtual / "objects" / "sha256" / digest[:2] / digest
        # Check staging first: an existing different partial upload must never be attached to restored metadata.
        for name, digest in manifest["staging"].items():
            target = _safe_path(data_dir, "attachments/upload-staging/" + name, create_parents=True)
            if target.exists() and _hash_file(target) != digest:
                raise ValueError("staging restore conflict")
        if identity:
            if vault_identity_writer is None or vault_identity_writer(identity) not in ("created", "already_present"):
                raise ValueError("vault_identity_conflict")
        for digest in manifest["originals"]:
            target = _safe_path(Path(archive), f"objects/sha256/{digest[:2]}/{digest}", create_parents=True)
            _install_verified(source(digest), target, digest, False)
        for name, digest in manifest["staging"].items():
            target = _safe_path(data_dir, "attachments/upload-staging/" + name, create_parents=True)
            _install_verified(source(digest), target, digest, False)
        for name, digest in manifest["files"].items():
            target = _safe_path(data_dir, name, create_parents=True)
            candidate = source(digest)
            install_digest = digest
            if name == "attachments/attachments.sqlite3" and purged:
                candidate = Path(tmp) / "purged-attachments.sqlite3"
                shutil.copyfile(source(digest), candidate)
                os.chmod(candidate, 0o600)
                with closing(sqlite3.connect(candidate)) as database:
                    database.execute("CREATE TABLE IF NOT EXISTS asset_purged(asset_id TEXT PRIMARY KEY REFERENCES assets(id))")
                    for deleted_hash in purged:
                        database.execute("INSERT OR IGNORE INTO asset_purged(asset_id) SELECT id FROM assets WHERE hash=?", (deleted_hash,))
                    database.commit()
                install_digest = _hash_file(candidate)
            conflict = _install_verified(candidate, target, install_digest, True)
            if conflict:
                conflicts.append(name)
    return {"status": "conflict" if conflicts else "restored", "conflicts": conflicts}


def restore_assets(data_dir, archive, store, key_service, asset_ids):
    """Restore only requested attachment originals and metadata, without replacing local data."""
    import fcntl
    data_dir, archive = Path(data_dir), Path(archive)
    if data_dir.is_symlink() or archive.is_symlink() or not archive.is_dir():
        raise ValueError("unsafe asset destination")
    if not isinstance(asset_ids, list) or len(set(asset_ids)) != len(asset_ids):
        raise ValueError("invalid asset list")
    for value in asset_ids:
        if not isinstance(value, str) or str(uuid.UUID(value)) != value:
            raise ValueError("invalid asset id")
    latest = _latest(store)
    if not latest:
        raise ValueError("asset backup unavailable")
    files = _unpack(_decrypt(store.read(latest[1]), key_service))
    purged = read_purged_ledger(store, key_service)
    purged.update(_snapshot_purge_markers(files))
    if ".momso-v3.json" not in files:
        raise ValueError("asset backup unavailable")
    manifest = json.loads(files[".momso-v3.json"])
    if not isinstance(manifest, dict) or manifest.get("version") != 3:
        raise ValueError("invalid backup manifest")
    descriptors = {}
    for item in manifest.get("descriptors", []):
        digest, _, _, _ = _object_descriptor(item)
        if digest in descriptors:
            raise ValueError("duplicate object descriptor")
        descriptors[digest] = item
    database_hash = manifest.get("files", {}).get("attachments/attachments.sqlite3")
    if database_hash not in descriptors:
        raise ValueError("asset backup unavailable")
    data_dir.mkdir(mode=0o700, parents=True, exist_ok=True)
    with tempfile.TemporaryDirectory(prefix=".momso-assets-restore-", dir=data_dir) as temp:
        virtual = Path(temp) / "virtual"
        restore_objects(virtual, store, key_service, [descriptors[database_hash]])
        snapshot = virtual / "objects" / "sha256" / database_hash[:2] / database_hash
        with closing(sqlite3.connect(f"{snapshot.as_uri()}?mode=ro", uri=True)) as source:
            records = []
            has_trash = source.execute("SELECT EXISTS(SELECT 1 FROM sqlite_master WHERE type='table' AND name='asset_trash')").fetchone()[0]
            has_media = source.execute("SELECT EXISTS(SELECT 1 FROM sqlite_master WHERE type='table' AND name='media_generation')").fetchone()[0]
            has_dates = source.execute("SELECT EXISTS(SELECT 1 FROM sqlite_master WHERE type='table' AND name='asset_dates')").fetchone()[0]
            trash = {}
            media = {}
            dates = {}
            purged_ids = set()
            for identifier in asset_ids:
                row = source.execute("SELECT id,hash,bytes,name,mime,kind FROM assets WHERE id=?", (identifier,)).fetchone()
                deleted = bool(row and row[1] in purged)
                if (not row or not re.fullmatch(r"[0-9a-f]{64}", row[1])
                        or (not deleted and (row[1] not in manifest.get("originals", []) or row[1] not in descriptors or descriptors[row[1]]["size"] != row[2]))):
                    raise ValueError("asset backup unavailable")
                if deleted:
                    purged_ids.add(identifier)
                records.append(row)
                if has_dates:
                    date = source.execute("SELECT created_at,source FROM asset_dates WHERE asset_id=?", (identifier,)).fetchone()
                    if date:
                        if not isinstance(date[0], str) or not date[0] or len(date[0]) > 64 or any(ord(c) < 32 for c in date[0]) or date[1] not in ('registered', 'file_modified'):
                            raise ValueError("invalid asset date")
                        dates[identifier] = date
                if has_trash:
                    deleted = source.execute("SELECT deleted_at FROM asset_trash WHERE asset_id=?", (identifier,)).fetchone()
                    if deleted:
                        if not isinstance(deleted[0], str) or not deleted[0] or len(deleted[0]) > 64 or any(ord(c) < 32 for c in deleted[0]):
                            raise ValueError("invalid asset trash marker")
                        trash[identifier] = deleted[0]
                if has_media:
                    item = source.execute("SELECT asset_id,prompt,prompt_source,provider,model,created_at,chat_id,message_id,lineage_id,version,version_of,version_how FROM media_generation WHERE asset_id=?", (identifier,)).fetchone()
                    if item:
                        if not isinstance(item[1], str) or len(item[1].encode('utf-8')) > 65536 or item[2] not in ('reported','userRequest') or item[3] not in ('codex','claude','gemini') or not isinstance(item[9], int) or item[9] < 1:
                            raise ValueError("invalid media record")
                        if any(not isinstance(item[i], str) or not item[i] or len(item[i]) > 200 or any(ord(c) < 32 for c in item[i]) for i in (5,6,7)) or (item[4] is not None and (not isinstance(item[4], str) or len(item[4]) > 200 or any(ord(c) < 32 for c in item[4]))):
                            raise ValueError("invalid media record")
                        if str(uuid.UUID(item[8])) != item[8] or (item[10] is not None and str(uuid.UUID(item[10])) != item[10]) or item[11] not in (None,'regenerated','edited'):
                            raise ValueError("invalid media record")
                        media[identifier] = item
        unique = {row[1] for row in records if row[0] not in purged_ids}
        restore_objects(virtual, store, key_service, [descriptors[digest] for digest in sorted(unique)])
        attachment_dir = _safe_path(data_dir, "attachments")
        database = _safe_path(data_dir, "attachments/attachments.sqlite3")
        if not attachment_dir.is_dir() or not database.is_file() or database.is_symlink():
            raise ValueError("asset database unavailable")
        if attachment_dir.stat().st_mode & 0o077 or database.stat().st_mode & 0o077:
            raise ValueError("unsafe asset database permissions")
        lock_fd = os.open(attachment_dir / "attachments.lock", os.O_RDWR | os.O_CREAT | getattr(os, "O_NOFOLLOW", 0), 0o600)
        try:
            deadline = time.monotonic() + 2.0
            while True:
                try:
                    fcntl.flock(lock_fd, fcntl.LOCK_EX | fcntl.LOCK_NB)
                    break
                except BlockingIOError:
                    remaining = deadline - time.monotonic()
                    if remaining <= 0:
                        raise BlockingIOError("asset restore busy") from None
                    time.sleep(min(0.05, remaining))
            with closing(sqlite3.connect(database)) as target:
                target.execute("CREATE TABLE IF NOT EXISTS asset_trash(asset_id TEXT PRIMARY KEY REFERENCES assets(id), deleted_at TEXT NOT NULL)")
                target.execute("CREATE TABLE IF NOT EXISTS asset_purged(asset_id TEXT PRIMARY KEY REFERENCES assets(id))")
                target.execute("CREATE TABLE IF NOT EXISTS asset_dates(asset_id TEXT PRIMARY KEY REFERENCES assets(id), created_at TEXT NOT NULL, source TEXT NOT NULL)")
                target.execute("CREATE TABLE IF NOT EXISTS media_generation(asset_id TEXT PRIMARY KEY REFERENCES assets(id),prompt TEXT NOT NULL,prompt_source TEXT NOT NULL,provider TEXT NOT NULL,model TEXT,created_at TEXT NOT NULL,chat_id TEXT NOT NULL,message_id TEXT NOT NULL,lineage_id TEXT NOT NULL,version INTEGER NOT NULL,version_of TEXT,version_how TEXT,UNIQUE(lineage_id,version))")
                target.execute("BEGIN IMMEDIATE")
                new_ids = set()
                for row in records:
                    already_purged = target.execute("SELECT EXISTS(SELECT 1 FROM assets JOIN asset_purged ON asset_purged.asset_id=assets.id WHERE assets.id=? OR assets.hash=?)", (row[0], row[1])).fetchone()[0]
                    if already_purged and row[0] not in purged_ids:
                        raise ValueError("asset permanently deleted")
                    existing = target.execute("SELECT id,hash,bytes FROM assets WHERE id=? OR hash=?", (row[0], row[1])).fetchall()
                    if existing and (len(existing) != 1 or existing[0] != row[:3]):
                        raise ValueError("asset restore conflict")
                    if existing and row[0] in purged_ids and not already_purged:
                        raise ValueError("asset restore conflict")
                    if not existing:
                        new_ids.add(row[0])
                for digest in sorted(unique):
                    source_path = virtual / "objects" / "sha256" / digest[:2] / digest
                    destination = _safe_path(archive, f"objects/sha256/{digest[:2]}/{digest}", create_parents=True)
                    _install_verified(source_path, destination, digest, False)
                for row in records:
                    target.execute("INSERT OR IGNORE INTO assets(id,hash,bytes,name,mime,kind) VALUES(?,?,?,?,?,?)", row)
                    if row[0] in purged_ids:
                        target.execute("INSERT OR IGNORE INTO asset_purged(asset_id) VALUES(?)", (row[0],))
                    if row[0] in new_ids and row[0] in dates:
                        target.execute("INSERT INTO asset_dates(asset_id,created_at,source) VALUES(?,?,?)", (row[0], *dates[row[0]]))
                    if row[0] in new_ids and row[0] in trash:
                        target.execute("INSERT INTO asset_trash(asset_id,deleted_at) VALUES(?,?)", (row[0], trash[row[0]]))
                    if row[0] in new_ids and row[0] in media:
                        target.execute("INSERT INTO media_generation VALUES(?,?,?,?,?,?,?,?,?,?,?,?)", media[row[0]])
                target.commit()
        finally:
            os.close(lock_fd)


def _latest(store):
    slots = [(store.modified(i), i) for i in range(SLOT_COUNT)]
    present = [(when, i) for when, i in slots if when is not None]
    return max(present) if present else None


def _finish_staged_backup(store, key_service, names, preserve_existing_slots=False, protected_slot=None, temp_root=None):
    """Adopt the temporary slot without overwriting any existing generation."""
    staged = store.read_versioned(SLOT_COUNT)
    if staged is None:
        return None
    if preserve_existing_slots and protected_slot == SLOT_COUNT:
        raise ValueError('restored_temporary_backup_requires_recovery')
    generation, blob = staged
    present = [(store.modified(i), i) for i in range(SLOT_COUNT)]
    try:
        verify_backup_blob(blob, store, key_service, names, temp_root=temp_root)
        if store.read_versioned(SLOT_COUNT) != staged:
            raise ValueError("backup_changed")
    except Exception:
        # Regular slots are never removed: replacement is atomic. An invalid
        # staged value can therefore be discarded without touching them.
        if protected_slot != SLOT_COUNT:
            store.delete_slot_if_generation(SLOT_COUNT, generation)
        raise
    for _, i in present:
        existing = store.read_versioned(i)
        if existing is not None and existing[1] == blob:
            store.delete_slot_if_generation(SLOT_COUNT, generation)
            return i
    missing = [i for when, i in present if when is None]
    if missing:
        target = missing[0]
    else:
        if preserve_existing_slots and protected_slot is None:
            raise ValueError("restore_source_unknown")
        eligible = [(when, i) for when, i in present if i != protected_slot]
        if not eligible:
            raise ValueError("no_unprotected_backup")
        _, target = min(eligible)
        previous = store.read_versioned(target)
        if previous is None or store.read_versioned(SLOT_COUNT) != staged:
            raise ValueError("backup_changed")
        store.replace_if_generation(target, previous[0], blob)
    if missing:
        store.create_slot_if_absent(target, blob)
    copy = store.read_versioned(target)
    if not copy or copy[1] != blob or store.read_versioned(SLOT_COUNT) != staged:
        raise ValueError("backup_changed")
    store.delete_slot_if_generation(SLOT_COUNT, generation)
    return target


def _stage_and_rotate(store, key_service, names, blob, preserve_existing_slots=False, protected_slot=None, temp_root=None):
    if store.read_versioned(SLOT_COUNT) is not None:
        raise ValueError("backup_stage_not_recovered")
    store.create_slot_if_absent(SLOT_COUNT, blob)
    _finish_staged_backup(store, key_service, names, preserve_existing_slots, protected_slot, temp_root)


def select_restore_candidate(store, key_service, names=DATABASE_AND_VAULT):
    """Prefer a verified regular slot; use the temporary slot only as last resort."""
    times = [(store.modified(i), i) for i in range(SLOT_COUNT)]
    regular = sorted(((when, i) for when, i in times if when is not None), reverse=True)
    candidates = [slot for _, slot in regular]
    if store.modified(SLOT_COUNT) is not None:
        candidates.append(SLOT_COUNT)
    last_error = None
    for slot in candidates:
        version = store.read_versioned(slot)
        if version is None:
            raise ValueError('backup_changed')
        try:
            verify_backup_blob(version[1], store, key_service, names)
        except (ValueError, InvalidTag, KeyError, FileNotFoundError) as error:
            if str(error) == 'backup_changed':
                raise
            last_error = error
            continue
        if store.read_versioned(slot) != version:
            raise ValueError('backup_changed')
        return slot, version
    if last_error is not None:
        raise last_error
    return None


def _decode_purged_ledger(encrypted, key_service):
    if encrypted is None:
        return set()
    if len(encrypted) > 2 * 1024 * 1024:
        raise ValueError("invalid purge ledger")
    ledger = json.loads(_decrypt(encrypted, key_service))
    if (not isinstance(ledger, dict) or set(ledger) != {"version", "digests"} or ledger["version"] != 1
            or not isinstance(ledger["digests"], list)
            or any(not isinstance(value, str) or not re.fullmatch(r"[0-9a-f]{64}", value) for value in ledger["digests"])
            or len(set(ledger["digests"])) != len(ledger["digests"])):
        raise ValueError("invalid purge ledger")
    return set(ledger["digests"])


def read_purged_ledger(store, key_service):
    """Fail closed on a malformed authoritative ledger, never guess it is empty."""
    return _decode_purged_ledger(store.read_purge_ledger(), key_service)


def _snapshot_purge_markers(files):
    values = json.loads(files.get(".momso-purged-originals.json", b"[]"))
    if (not isinstance(values, list) or any(not isinstance(value, str) or not re.fullmatch(r"[0-9a-f]{64}", value) for value in values)
            or len(set(values)) != len(values)):
        raise ValueError("invalid purge markers")
    return set(values)


def record_purged_original(store, key_service, digest):
    """Conditionally append a marker. Caller must hold the shared backup fence."""
    if not isinstance(digest, str) or not re.fullmatch(r"[0-9a-f]{64}", digest):
        raise ValueError("invalid original digest")
    versioned = store.read_purge_ledger_versioned()
    generation, encrypted = versioned if versioned is not None else ("0", None)
    values = _decode_purged_ledger(encrypted, key_service)
    if digest in values:
        return False
    values.add(digest)
    encoded = json.dumps({"version": 1, "digests": sorted(values)}, separators=(",", ":")).encode("ascii")
    store.write_purge_ledger_if_generation(generation, _encrypt(encoded, key_service))
    return True


def plan_original_purge(store, key_service, digest):
    """Inspect every backup slot before any destructive action.

    This does not delete or rewrite anything. A hash that is also used by a
    database or upload staging file must never be removed as an original.
    """
    if not isinstance(digest, str) or not re.fullmatch(r"[0-9a-f]{64}", digest):
        raise ValueError("invalid original digest")
    slots = []
    all_slots = []
    chunks = set()
    owners = {}
    for slot in range(SLOT_COUNT):
        if store.modified(slot) is None:
            continue
        all_slots.append(slot)
        contents = _unpack(_decrypt(store.read(slot), key_service))
        if ".momso-v3.json" not in contents:
            raise ValueError("unsupported backup generation")
        manifest = json.loads(contents[".momso-v3.json"])
        if (not isinstance(manifest, dict) or set(manifest) != {"version", "files", "staging", "originals", "descriptors"} or manifest.get("version") != 3
                or not isinstance(manifest.get("files"), dict)
                or not isinstance(manifest.get("staging"), dict)
                or not isinstance(manifest.get("originals"), list)
                or not isinstance(manifest.get("descriptors"), list)):
            raise ValueError("invalid backup manifest")
        descriptors = {}
        for entry in manifest["descriptors"]:
            entry_hash, _, count, identifier = _object_descriptor(entry)
            if entry_hash in descriptors or (identifier in owners and owners[identifier] != entry_hash):
                raise ValueError("conflicting backup descriptors")
            descriptors[entry_hash] = (identifier, count)
            owners[identifier] = entry_hash
        references = list(manifest["files"].values()) + list(manifest["staging"].values())
        if any(value not in descriptors for value in references + manifest["originals"]):
            raise ValueError("incomplete backup manifest")
        if digest in references:
            raise ValueError("original shared with backup data")
        if digest in manifest["originals"]:
            identifier, count = descriptors[digest]
            slots.append(slot)
            chunks.update((identifier, i) for i in range(count))
    return {"slots": slots, "allSlots": all_slots, "chunks": sorted(chunks)}


def without_original(contents, digest):
    """Prepare a replacement v3 slot, preserving all unrelated entries.

    Callers must validate every slot with plan_original_purge and use a
    generation-conditional write; this function itself performs no I/O.
    """
    if not isinstance(digest, str) or not re.fullmatch(r"[0-9a-f]{64}", digest):
        raise ValueError("invalid original digest")
    return without_originals(contents, {digest})


def without_originals(contents, digests):
    if (not isinstance(digests, set) or any(not isinstance(value, str) or not re.fullmatch(r"[0-9a-f]{64}", value) for value in digests)):
        raise ValueError("invalid original digest")
    if not isinstance(contents, dict) or ".momso-v3.json" not in contents:
        raise ValueError("unsupported backup generation")
    manifest = json.loads(contents[".momso-v3.json"])
    if (not isinstance(manifest, dict) or set(manifest) != {"version", "files", "staging", "originals", "descriptors"} or manifest.get("version") != 3
            or not isinstance(manifest.get("files"), dict)
            or not isinstance(manifest.get("staging"), dict)
            or not isinstance(manifest.get("originals"), list)
            or not isinstance(manifest.get("descriptors"), list)):
        raise ValueError("invalid backup manifest")
    descriptors = {}
    for item in manifest["descriptors"]:
        item_hash, _, _, _ = _object_descriptor(item)
        if item_hash in descriptors:
            raise ValueError("duplicate backup descriptor")
        descriptors[item_hash] = item
    references = list(manifest["files"].values()) + list(manifest["staging"].values())
    if any(value not in descriptors for value in references + manifest["originals"]):
        raise ValueError("incomplete backup manifest")
    if digests.intersection(references):
        raise ValueError("original shared with backup data")
    updated = dict(manifest)
    updated["originals"] = [value for value in manifest["originals"] if value not in digests]
    updated["descriptors"] = [item for item in manifest["descriptors"] if item["sha256"] not in digests]
    result = dict(contents)
    result[".momso-v3.json"] = json.dumps(updated, sort_keys=True, separators=(",", ":")).encode("utf-8")
    purged = _snapshot_purge_markers(result)
    result[".momso-purged-originals.json"] = json.dumps(sorted(purged | digests), separators=(",", ":")).encode("ascii")
    return result


def replace_slot_without_original(store, key_service, slot, digest):
    """Conditionally rewrite one slot. Never removes a chunk or local original."""
    versioned = store.read_versioned(slot)
    if versioned is None:
        return False
    generation, encrypted = versioned
    before = _unpack(_decrypt(encrypted, key_service))
    after = without_original(before, digest)
    if after == before:
        return False
    store.replace_if_generation(slot, generation, _encrypt(_pack(after), key_service))
    return True


def read_purge_journal(directory):
    """Read a bounded, host-owned journal; a symlink is never followed."""
    directory = Path(directory)
    if directory.is_symlink() or not directory.is_dir():
        raise ValueError("unsafe purge journal directory")
    path = directory / "original-purge.json"
    if path.is_symlink():
        raise ValueError("unsafe purge journal")
    flags = os.O_RDONLY | getattr(os, "O_NOFOLLOW", 0) | getattr(os, "O_CLOEXEC", 0)
    try:
        fd = os.open(path, flags)
    except FileNotFoundError:
        return None
    try:
        if not stat.S_ISREG(os.fstat(fd).st_mode):
            raise ValueError("invalid purge journal")
        with os.fdopen(fd, "rb", closefd=False) as source:
            value = source.read(1024 * 1024 + 1)
        if len(value) > 1024 * 1024:
            raise ValueError("invalid purge journal")
        return json.loads(value)
    finally:
        os.close(fd)


def write_purge_journal(directory, journal):
    """Atomically persist the journal before changing any backup slot."""
    directory = Path(directory)
    if directory.is_symlink() or not directory.is_dir():
        raise ValueError("unsafe purge journal directory")
    value = json.dumps(journal, sort_keys=True, separators=(",", ":")).encode("utf-8")
    if len(value) > 1024 * 1024:
        raise ValueError("invalid purge journal")
    path = directory / "original-purge.json"
    if path.is_symlink():
        raise ValueError("unsafe purge journal")
    fd, temporary = tempfile.mkstemp(prefix=".original-purge-", dir=directory)
    try:
        with os.fdopen(fd, "wb") as output:
            output.write(value)
            output.flush()
            os.fsync(output.fileno())
        os.replace(temporary, path)
        if hasattr(os, "O_DIRECTORY"):
            parent = os.open(directory, os.O_RDONLY | os.O_DIRECTORY)
            try:
                os.fsync(parent)
            finally:
                os.close(parent)
    finally:
        if os.path.exists(temporary):
            os.unlink(temporary)


def resume_original_slot_rewrite(store, key_service, digest, journal, persist):
    """Prepare or resume slot rewrites while the caller holds a global backup fence.

    `persist` must durably write the entire journal before returning. No local
    original or encrypted chunk is removed here. This primitive is not a
    complete purge: the caller must enforce the fence across every backup and
    restore writer, process old object generations, and handle local metadata.
    """
    if not isinstance(digest, str) or not re.fullmatch(r"[0-9a-f]{64}", digest):
        raise ValueError("invalid original digest")
    if journal is None:
        plan = plan_original_purge(store, key_service, digest)
        journal = _new_purge_journal(digest, plan)
        persist(journal)
    _validate_purge_journal(digest, journal)
    if journal["version"] != 2:
        raise ValueError("purge journal requires review")
    for slot in journal["slots"]:
        # Even a completed slot is checked again; the fence must prevent a new
        # writer from restoring an old manifest between this check and commit.
        replace_slot_without_original(store, key_service, slot, digest)
        if slot not in journal["completed"]:
            journal["completed"] = [*journal["completed"], slot]
            persist(journal)
    if plan_original_purge(store, key_service, digest)["slots"]:
        raise ValueError("purge fence violated")
    return journal


def _validate_purge_journal(digest, journal):
    if (not isinstance(journal, dict) or type(journal.get("version")) is not int or journal.get("version") not in (1, 2)
            or set(journal) != ({"version", "digest", "slots", "chunks", "completed", "planHash"} if journal.get("version") == 2 else {"version", "digest", "slots", "chunks", "completed"})
            or journal["digest"] != digest
            or not isinstance(journal["slots"], list) or not isinstance(journal["chunks"], list)
            or not isinstance(journal["completed"], list)
            or any(type(slot) is not int or not 0 <= slot < SLOT_COUNT for slot in journal["slots"])
            or len(set(journal["slots"])) != len(journal["slots"])
            or any(type(slot) is not int or slot not in journal["slots"] for slot in journal["completed"])
            or len(set(journal["completed"])) != len(journal["completed"])
            or any(not isinstance(item, list) or len(item) != 2
                   or not isinstance(item[0], str) or not re.fullmatch(r"[0-9a-f]{32}", item[0])
                   or type(item[1]) is not int or item[1] < 0 for item in journal["chunks"])):
        raise ValueError("invalid purge journal")
    if journal["version"] == 2 and journal["planHash"] != _purge_plan_hash(journal):
        raise ValueError("invalid purge journal")


def _purge_plan_hash(journal):
    plan = {key: journal[key] for key in ("version", "digest", "slots", "chunks")}
    return hashlib.sha256(json.dumps(plan, sort_keys=True, separators=(",", ":")).encode("ascii")).hexdigest()


def _new_purge_journal(digest, plan):
    journal = {"version": 2, "digest": digest, "slots": plan["allSlots"],
               "chunks": [list(item) for item in plan["chunks"]], "completed": []}
    journal["planHash"] = _purge_plan_hash(journal)
    return journal


def prepare_original_backup_purge(store, key_service, digest, journal_directory):
    """Publish the deletion ledger and rewrite slots under a caller-owned fence.

    This does not remove local originals or cloud chunks and is not a complete
    purge. The caller must hold both the per-VM lock and the shared cloud fence
    throughout this operation, including crash recovery.
    """
    if not isinstance(digest, str) or not re.fullmatch(r"[0-9a-f]{64}", digest):
        raise ValueError("invalid original digest")
    journal = read_purge_journal(journal_directory)
    current = plan_original_purge(store, key_service, digest)
    if journal is None:
        journal = _new_purge_journal(digest, current)
        write_purge_journal(journal_directory, journal)
    _validate_purge_journal(digest, journal)
    if journal["version"] != 2:
        raise ValueError("purge journal requires review")
    if not set(current["allSlots"]).issubset(set(journal["slots"])):
        raise ValueError("purge fence violated")
    if not set(current["chunks"]).issubset({tuple(item) for item in journal["chunks"]}):
        raise ValueError("purge fence violated")
    record_purged_original(store, key_service, digest)
    return resume_original_slot_rewrite(store, key_service, digest, journal,
                                        lambda value: write_purge_journal(journal_directory, value))


def erase_prepared_backup_chunks(store, key_service, digest, journal):
    """Remove recorded chunk generations only under the shared backup fence.

    Older or soft-deleted slot generations may contain additional chunk names;
    while any exist, do not delete anything or claim completion. The caller is
    responsible for local originals, fence recovery and displaying retention.
    """
    _validate_purge_journal(digest, journal)
    if journal["version"] != 2:
        raise ValueError("purge journal requires review")
    if digest not in read_purged_ledger(store, key_service):
        raise ValueError("purge ledger missing")
    if set(journal["completed"]) != set(journal["slots"]):
        raise ValueError("purge slots incomplete")
    if plan_original_purge(store, key_service, digest)["slots"]:
        raise ValueError("purge fence violated")
    recorded = {tuple(item) for item in journal["chunks"]}
    historical = False
    unknown_history = False
    retained_history = []
    for slot in range(SLOT_COUNT):
        versioned = store.read_versioned(slot)
        current = versioned[0] if versioned else None
        versions = store.list_slot_generations(slot)
        retained = store.list_slot_generations(slot, soft_deleted=True)
        if current is not None and not any(item.get("generation") == current for item in versions):
            raise ValueError("incomplete backup generation listing")
        noncurrent = [item for item in versions if item["generation"] != current]
        if retained or noncurrent:
            historical = True
            unknown_history = unknown_history or bool(noncurrent)
            retained_history.extend(retained)
        if versioned:
            contents = _unpack(_decrypt(versioned[1], key_service))
            if ".momso-v3.json" not in contents:
                raise ValueError("unsupported backup generation")
            manifest = json.loads(contents[".momso-v3.json"])
            for item in manifest.get("descriptors", []):
                _, _, count, identifier = _object_descriptor(item)
                if any((identifier, index) in recorded for index in range(count)):
                    raise ValueError("purge chunk still referenced")
    if historical:
        return {"state": "retained", "reason": "historical_backup",
                "retentionUntil": None if unknown_history else _latest_retention_time(retained_history)}
    versions_by_chunk = {}
    # Check that all required listing permissions work before deleting anything.
    for identifier, index in sorted(recorded):
        versions_by_chunk[(identifier, index)] = store.list_chunk_generations(identifier, index)
        store.list_chunk_generations(identifier, index, soft_deleted=True)
    for (identifier, index), versions in versions_by_chunk.items():
        for item in versions:
            store.delete_chunk_generation(identifier, index, item["generation"])
    pending = []
    for identifier, index in sorted(recorded):
        pending.extend(store.list_chunk_generations(identifier, index))
        pending.extend(store.list_chunk_generations(identifier, index, soft_deleted=True))
    return {"state": "retained" if pending else "deleted", "reason": "cloud_retention" if pending else None,
            "retentionUntil": _latest_retention_time(pending)}


def _latest_retention_time(items):
    times = [item.get("hardDeleteTime") for item in items]
    if not times or not all(times):
        return None
    try:
        parsed = [datetime.datetime.fromisoformat(value.replace("Z", "+00:00")) for value in times]
        if any(value.tzinfo is None for value in parsed):
            raise ValueError()
        return max(value.astimezone(datetime.timezone.utc) for value in parsed).isoformat().replace("+00:00", "Z")
    except (AttributeError, TypeError, ValueError):
        raise ValueError("invalid retention time") from None


def backup(data_dir, store, key_service, names, temp_root=None, vault_identity=None, archive=None, quiescent=False, memory_files=None, preserve_existing_slots=False, protected_slot=None):
    """Backup changed data, rotating fixed slots. The caller schedules daily."""
    data_dir = Path(data_dir)
    if archive is not None and not quiescent:
        raise ValueError("quiescence_required")
    _finish_staged_backup(store, key_service, names, preserve_existing_slots, protected_slot, temp_root)
    if archive is not None:
        return _backup_v3(data_dir, Path(archive), store, key_service, names, vault_identity, memory_files, preserve_existing_slots, protected_slot, temp_root)
    with tempfile.TemporaryDirectory(prefix="momso-backup-", dir=temp_root) as tmp:
        files = _snapshot(data_dir, Path(tmp), names)
        latest = _latest(store)
        previous_payload = _decrypt(store.read(latest[1]), key_service) if latest else None
        if archive is not None:
            previous_files = _unpack(previous_payload) if previous_payload else {}
            previous = json.loads(previous_files.get(".momso-objects.json", b"[]"))
            if not isinstance(previous, list):
                raise ValueError("invalid object manifest")
            descriptors = backup_objects(archive, store, key_service, previous)
            files[".momso-objects.json"] = json.dumps(descriptors, sort_keys=True, separators=(",", ":")).encode("utf-8")
        if not files:
            return "nothing_to_backup"
        if any(name == "vault.age" or name.endswith("/vault.age") for name in files):
            if not vault_identity:
                raise ValueError("vault_identity_unavailable")
            # The identity is inside the encrypted envelope, never the temp DB.
            files[".momso-vault-identity"] = vault_identity
        payload = _pack(files)
        if previous_payload:
            if hashlib.sha256(previous_payload).digest() == hashlib.sha256(payload).digest():
                return "unchanged"
        _stage_and_rotate(store, key_service, names, _encrypt(payload, key_service),
                          preserve_existing_slots, protected_slot, temp_root)
        return "uploaded"


def restore(data_dir, store, key_service, names=DATABASE_AND_VAULT, vault_identity_writer=None, archive=None, slot=None):
    """Restore only missing destinations; preserve conflicts separately."""
    data_dir = Path(data_dir)
    latest = _latest(store) if slot is None else (True, slot)
    if not latest:
        return {"status": "no_backup", "conflicts": []}
    files = _unpack(_decrypt(store.read(latest[1]), key_service))
    ledger = read_purged_ledger(store, key_service)
    if ledger:
        files = without_originals(files, ledger)
    identity = files.pop(".momso-vault-identity", None)
    purged = _snapshot_purge_markers(files)
    files.pop(".momso-purged-originals.json", None)
    v3 = files.pop(".momso-v3.json", None)
    if v3 is not None:
        if files:
            raise ValueError("unexpected backup entry")
        if archive is None:
            raise ValueError("archive_destination_required")
        return _restore_v3(data_dir, archive, store, key_service, json.loads(v3), names, identity, vault_identity_writer, purged)
    if purged:
        raise ValueError("unsupported purge markers")
    objects = files.pop(".momso-objects.json", None)
    if not set(files).issubset(set(names)):
        raise ValueError("unexpected backup entry")
    if objects is not None:
        if archive is None:
            raise ValueError("archive_destination_required")
        descriptors = json.loads(objects)
        if not isinstance(descriptors, list):
            raise ValueError("invalid object manifest")
        restore_objects(archive, store, key_service, descriptors)
    if any(name == "vault.age" or name.endswith("/vault.age") for name in files):
        if not identity:
            raise ValueError("vault_identity_unavailable")
        if any(not _safe_path(data_dir, name).exists() for name in files if name == "vault.age" or name.endswith("/vault.age")):
            if vault_identity_writer is None:
                raise ValueError("vault_identity_writer_unavailable")
            if vault_identity_writer(identity) not in ("created", "already_present"):
                raise ValueError("vault_identity_conflict")
    data_dir.mkdir(mode=0o700, parents=True, exist_ok=True)
    conflicts = []
    for name, content in files.items():
        target = _safe_path(data_dir, name, create_parents=True)
        fd, temporary = tempfile.mkstemp(prefix=".momso-restore-", dir=target.parent)
        try:
            with os.fdopen(fd, "wb") as output:
                output.write(content)
                output.flush()
                os.fsync(output.fileno())
            try:
                # A hard link is created only if the destination is still
                # absent, even if another process creates it mid-restore.
                os.link(temporary, target)
                continue
            except FileExistsError:
                pass
            # O_EXCL keeps repeated restores and unrelated files intact.
            conflicts.append(name)
            n = 0
            while True:
                suffix = ".recovered" if n == 0 else f".recovered-{n}"
                try:
                    fd = os.open(str(target) + suffix, os.O_WRONLY | os.O_CREAT | os.O_EXCL, 0o600)
                    break
                except FileExistsError:
                    n += 1
            with os.fdopen(fd, "wb") as output:
                output.write(content)
        finally:
            if os.path.exists(temporary):
                os.unlink(temporary)
    return {"status": "conflict" if conflicts else "restored", "conflicts": conflicts}
