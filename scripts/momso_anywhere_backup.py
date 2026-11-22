"""Encrypted, fixed-slot backup primitives for a customer-owned VM.

The storage and key interfaces deliberately contain no cloud credentials.  The
installer must supply implementations and schedule this module before it can
be used as a production backup.
"""
import hashlib
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
import uuid
from contextlib import closing
from pathlib import Path, PurePosixPath

from cryptography.hazmat.primitives.ciphers.aead import AESGCM

SLOT_COUNT = 7
FORMAT = b"momso-anywhere-backup-v1"
CHUNK_BYTES = 4 * 1024 * 1024
DATABASE_AND_VAULT = (
    "server.sqlite3", "class-student-private.sqlite3", "vault.age",
    "attachments/attachments.sqlite3", "saved-logins/vault.age",
    "provider-credentials/claude/vault.age", "provider-credentials/gemini/vault.age",
    "codex/auth.json", "github-repository.json", "chat-export-journal.json",
)


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
    descriptor = os.open(path, flags)
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


def backup_objects(archive, store, key_service, previous=()):
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


def _backup_v3(data_dir, archive, store, key_service, names, vault_identity):
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
        originals = backup_objects(archive, store, key_service, previous)
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
        times = [(store.modified(i), i) for i in range(SLOT_COUNT)]
        missing = [i for when, i in times if when is None]
        slot = missing[0] if missing else min(times)[1]
        store.write(slot, _encrypt(payload, key_service))
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


def _restore_v3(data_dir, archive, store, key_service, manifest, names, identity, vault_identity_writer):
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
            conflict = _install_verified(source(digest), target, digest, True)
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
            trash = {}
            media = {}
            for identifier in asset_ids:
                row = source.execute("SELECT id,hash,bytes,name,mime,kind FROM assets WHERE id=?", (identifier,)).fetchone()
                if not row or not re.fullmatch(r"[0-9a-f]{64}", row[1]) or row[1] not in manifest.get("originals", []) or row[1] not in descriptors or descriptors[row[1]]["size"] != row[2]:
                    raise ValueError("asset backup unavailable")
                records.append(row)
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
        unique = {row[1] for row in records}
        restore_objects(virtual, store, key_service, [descriptors[digest] for digest in sorted(unique)])
        attachment_dir = _safe_path(data_dir, "attachments")
        database = _safe_path(data_dir, "attachments/attachments.sqlite3")
        if not attachment_dir.is_dir() or not database.is_file() or database.is_symlink():
            raise ValueError("asset database unavailable")
        if attachment_dir.stat().st_mode & 0o077 or database.stat().st_mode & 0o077:
            raise ValueError("unsafe asset database permissions")
        lock_fd = os.open(attachment_dir / "attachments.lock", os.O_RDWR | os.O_CREAT | getattr(os, "O_NOFOLLOW", 0), 0o600)
        try:
            fcntl.flock(lock_fd, fcntl.LOCK_EX | fcntl.LOCK_NB)
            with closing(sqlite3.connect(database)) as target:
                target.execute("CREATE TABLE IF NOT EXISTS asset_trash(asset_id TEXT PRIMARY KEY REFERENCES assets(id), deleted_at TEXT NOT NULL)")
                target.execute("CREATE TABLE IF NOT EXISTS media_generation(asset_id TEXT PRIMARY KEY REFERENCES assets(id),prompt TEXT NOT NULL,prompt_source TEXT NOT NULL,provider TEXT NOT NULL,model TEXT,created_at TEXT NOT NULL,chat_id TEXT NOT NULL,message_id TEXT NOT NULL,lineage_id TEXT NOT NULL,version INTEGER NOT NULL,version_of TEXT,version_how TEXT,UNIQUE(lineage_id,version))")
                target.execute("BEGIN IMMEDIATE")
                new_ids = set()
                for row in records:
                    existing = target.execute("SELECT id,hash,bytes FROM assets WHERE id=? OR hash=?", (row[0], row[1])).fetchall()
                    if existing and (len(existing) != 1 or existing[0] != row[:3]):
                        raise ValueError("asset restore conflict")
                    if not existing:
                        new_ids.add(row[0])
                for digest in sorted(unique):
                    source_path = virtual / "objects" / "sha256" / digest[:2] / digest
                    destination = _safe_path(archive, f"objects/sha256/{digest[:2]}/{digest}", create_parents=True)
                    _install_verified(source_path, destination, digest, False)
                for row in records:
                    target.execute("INSERT OR IGNORE INTO assets(id,hash,bytes,name,mime,kind) VALUES(?,?,?,?,?,?)", row)
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


def backup(data_dir, store, key_service, names, temp_root=None, vault_identity=None, archive=None, quiescent=False):
    """Backup changed data, rotating fixed slots. The caller schedules daily."""
    data_dir = Path(data_dir)
    if archive is not None and not quiescent:
        raise ValueError("quiescence_required")
    if archive is not None:
        return _backup_v3(data_dir, Path(archive), store, key_service, names, vault_identity)
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
        times = [(store.modified(i), i) for i in range(SLOT_COUNT)]
        missing = [i for when, i in times if when is None]
        slot = missing[0] if missing else min(times)[1]
        store.write(slot, _encrypt(payload, key_service))
        return "uploaded"


def restore(data_dir, store, key_service, names=DATABASE_AND_VAULT, vault_identity_writer=None, archive=None):
    """Restore only missing destinations; preserve conflicts separately."""
    data_dir = Path(data_dir)
    latest = _latest(store)
    if not latest:
        return {"status": "no_backup", "conflicts": []}
    files = _unpack(_decrypt(store.read(latest[1]), key_service))
    identity = files.pop(".momso-vault-identity", None)
    v3 = files.pop(".momso-v3.json", None)
    if v3 is not None:
        if files:
            raise ValueError("unexpected backup entry")
        if archive is None:
            raise ValueError("archive_destination_required")
        return _restore_v3(data_dir, archive, store, key_service, json.loads(v3), names, identity, vault_identity_writer)
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
