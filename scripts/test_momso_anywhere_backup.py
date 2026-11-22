import os
import hashlib
import sqlite3
import tempfile
import unittest
from contextlib import closing
from pathlib import Path
from types import SimpleNamespace

from cryptography.hazmat.primitives.ciphers.aead import AESGCM
from cryptography.exceptions import InvalidTag

import momso_anywhere_backup as backup
from momso_anywhere_backup_google import CloudKms, CloudStorage
from momso_anywhere_backup_provision import GoogleBackupProvisioner
from momso_server_install import InstallError
from momso_anywhere_vault_identity import rewrap_for_host


class FakeKeys:
    def __init__(self):
        self.key = AESGCM.generate_key(bit_length=256)

    def wrap(self, value):
        nonce = os.urandom(12)
        return nonce + AESGCM(self.key).encrypt(nonce, value, b"fake-kms")

    def unwrap(self, value):
        return AESGCM(self.key).decrypt(value[:12], value[12:], b"fake-kms")


class FakeStore:
    def __init__(self):
        self.objects = {}
        self.chunks = {}
        self.clock = 0

    def modified(self, slot):
        return self.objects[slot][0] if slot in self.objects else None

    def read(self, slot):
        return self.objects[slot][1]

    def write(self, slot, content):
        self.clock += 1
        self.objects[slot] = (self.clock, content)

    def write_chunk(self, identifier, index, content):
        key = (identifier, index)
        if key in self.chunks:
            raise ValueError("chunk already exists")
        self.chunks[key] = content

    def read_chunk(self, identifier, index):
        return self.chunks[(identifier, index)]

    def chunk_matches(self, identifier, index, size, md5_hex):
        value = self.chunks.get((identifier, index))
        return value is not None and len(value) == size and hashlib.md5(value, usedforsecurity=False).hexdigest() == md5_hex


class AnywhereBackupTests(unittest.TestCase):
    def setUp(self):
        self.temp = tempfile.TemporaryDirectory()
        self.addCleanup(self.temp.cleanup)
        self.root = Path(self.temp.name)
        self.data = self.root / "data"
        self.data.mkdir()
        self.store = FakeStore()
        self.keys = FakeKeys()
        self.names = ["server.sqlite3", "vault.age"]

    def write_db(self, value):
        with closing(sqlite3.connect(self.data / "server.sqlite3")) as db:
            db.execute("CREATE TABLE IF NOT EXISTS messages (body TEXT)")
            db.execute("INSERT INTO messages VALUES (?)", (value,))
            db.commit()

    def test_v3_restores_originals_staging_and_database_without_overwrite(self):
        self.write_db("lesson")
        archive = self.data / "archive"
        content = b"image fixture" * 900000
        digest = hashlib.sha256(content).hexdigest()
        original = archive / "objects" / "sha256" / digest[:2] / digest
        original.parent.mkdir(parents=True)
        original.write_bytes(content)
        stage = self.data / "attachments" / "upload-staging" / "12345678-1234-1234-1234-123456789abc"
        stage.parent.mkdir(parents=True)
        stage.write_bytes(b"unfinished upload")
        self.assertEqual(backup.backup(self.data, self.store, self.keys, self.names, archive=archive, quiescent=True), "uploaded")
        for ciphertext in self.store.chunks.values():
            self.assertNotIn(b"image fixture", ciphertext)
            self.assertNotIn(b"unfinished upload", ciphertext)
            self.assertNotIn(b"lesson", ciphertext)
        self.assertEqual(backup.backup(self.data, self.store, self.keys, self.names, archive=archive, quiescent=True), "unchanged")
        destination = self.root / "new"
        result = backup.restore(destination, self.store, self.keys, names=self.names, archive=destination / "archive")
        self.assertEqual(result["status"], "restored")
        self.assertEqual((destination / "archive" / original.relative_to(archive)).read_bytes(), content)
        self.assertEqual((destination / "attachments" / "upload-staging" / stage.name).read_bytes(), b"unfinished upload")
        with closing(sqlite3.connect(destination / "server.sqlite3")) as db:
            self.assertEqual(db.execute("SELECT body FROM messages").fetchone()[0], "lesson")

    def test_v3_rejects_staging_conflict_without_touching_existing_data(self):
        self.write_db("lesson")
        archive = self.data / "archive"
        archive.mkdir()
        upload_id = "12345678-1234-1234-1234-123456789abc"
        stage = self.data / "attachments" / "upload-staging" / upload_id
        stage.parent.mkdir(parents=True)
        stage.write_bytes(b"remote partial upload")
        backup.backup(self.data, self.store, self.keys, self.names, archive=archive, quiescent=True)
        destination = self.root / "new"
        local_stage = destination / "attachments" / "upload-staging" / upload_id
        local_stage.parent.mkdir(parents=True)
        local_stage.write_bytes(b"local partial upload")
        with self.assertRaisesRegex(ValueError, "staging restore conflict"):
            backup.restore(destination, self.store, self.keys, names=self.names, archive=destination / "archive")
        self.assertEqual(local_stage.read_bytes(), b"local partial upload")
        self.assertFalse((destination / "server.sqlite3").exists())

    @unittest.skipUnless(os.name == 'posix', 'requires Linux file locking')
    def test_restore_requested_assets_keeps_local_data_and_is_idempotent(self):
        identifier = "12345678-1234-1234-1234-123456789abc"
        content = b"fake attachment"
        digest = hashlib.sha256(content).hexdigest()
        archive = self.data / "archive"
        original = archive / "objects" / "sha256" / digest[:2] / digest
        original.parent.mkdir(parents=True)
        original.write_bytes(content)
        database = self.data / "attachments" / "attachments.sqlite3"
        database.parent.mkdir()
        schema = "CREATE TABLE assets(id TEXT PRIMARY KEY,hash TEXT UNIQUE NOT NULL,bytes INTEGER NOT NULL,name TEXT NOT NULL,mime TEXT NOT NULL,kind TEXT NOT NULL)"
        with closing(sqlite3.connect(database)) as db:
            db.execute(schema)
            db.execute("CREATE TABLE asset_trash(asset_id TEXT PRIMARY KEY,deleted_at TEXT NOT NULL)")
            db.execute("INSERT INTO assets VALUES(?,?,?,?,?,?)", (identifier, digest, len(content), "example.txt", "text/plain", "document"))
            db.execute("INSERT INTO asset_trash VALUES(?,?)", (identifier, "2026-11-22T00:00:00Z"))
            db.commit()
        backup.backup(self.data, self.store, self.keys, ["attachments/attachments.sqlite3"], archive=archive, quiescent=True)
        destination = self.root / "destination"
        (destination / "archive").mkdir(parents=True)
        (destination / "attachments").mkdir(mode=0o700)
        target_db = destination / "attachments" / "attachments.sqlite3"
        with closing(sqlite3.connect(target_db)) as db:
            db.execute(schema)
            db.commit()
        os.chmod(target_db, 0o600)
        for _ in range(2):
            backup.restore_assets(destination, destination / "archive", self.store, self.keys, [identifier])
        with closing(sqlite3.connect(target_db)) as db:
            self.assertEqual(db.execute("SELECT id,hash FROM assets").fetchall(), [(identifier, digest)])
            self.assertEqual(db.execute("SELECT asset_id FROM asset_trash").fetchall(), [(identifier,)])
            db.execute("DELETE FROM asset_trash WHERE asset_id=?", (identifier,))
            db.commit()
        backup.restore_assets(destination, destination / "archive", self.store, self.keys, [identifier])
        with closing(sqlite3.connect(target_db)) as db:
            self.assertEqual(db.execute("SELECT asset_id FROM asset_trash").fetchall(), [])
        self.assertEqual((destination / "archive/objects/sha256" / digest[:2] / digest).read_bytes(), content)
        with closing(sqlite3.connect(target_db)) as db:
            db.execute("UPDATE assets SET hash=? WHERE id=?", ("c" * 64, identifier))
            db.commit()
        with self.assertRaisesRegex(ValueError, "asset restore conflict"):
            backup.restore_assets(destination, destination / "archive", self.store, self.keys, [identifier])
        with closing(sqlite3.connect(target_db)) as db:
            self.assertEqual(db.execute("SELECT hash FROM assets WHERE id=?", (identifier,)).fetchone()[0], "c" * 64)
        with self.assertRaisesRegex(ValueError, "asset backup unavailable"):
            backup.restore_assets(destination, destination / "archive", self.store, self.keys, ["abcdef12-1234-1234-1234-123456789abc"])
        if os.name == "posix":
            import fcntl
            with (destination / "attachments/attachments.lock").open("a+b") as lock:
                fcntl.flock(lock, fcntl.LOCK_EX | fcntl.LOCK_NB)
                with self.assertRaises(BlockingIOError):
                    backup.restore_assets(destination, destination / "archive", self.store, self.keys, [identifier])

    def test_restore_asset_preserves_generated_metadata_without_overwriting_local(self):
        identifier = "12345678-1234-1234-1234-123456789abc"
        lineage = "12345678-1234-1234-1234-123456789abd"
        content = b"synthetic image bytes"
        digest = hashlib.sha256(content).hexdigest()
        archive = self.data / "archive"
        original = archive / "objects" / "sha256" / digest[:2] / digest
        original.parent.mkdir(parents=True)
        original.write_bytes(content)
        database = self.data / "attachments" / "attachments.sqlite3"
        database.parent.mkdir()
        schema = "CREATE TABLE assets(id TEXT PRIMARY KEY,hash TEXT UNIQUE NOT NULL,bytes INTEGER NOT NULL,name TEXT NOT NULL,mime TEXT NOT NULL,kind TEXT NOT NULL)"
        media_schema = "CREATE TABLE media_generation(asset_id TEXT PRIMARY KEY,prompt TEXT NOT NULL,prompt_source TEXT NOT NULL,provider TEXT NOT NULL,model TEXT,created_at TEXT NOT NULL,chat_id TEXT NOT NULL,message_id TEXT NOT NULL,lineage_id TEXT NOT NULL,version INTEGER NOT NULL,version_of TEXT,version_how TEXT,UNIQUE(lineage_id,version))"
        with closing(sqlite3.connect(database)) as db:
            db.execute(schema)
            db.execute(media_schema)
            db.execute("INSERT INTO assets VALUES(?,?,?,?,?,?)", (identifier,digest,len(content),"image.png","image/png","image"))
            db.execute("INSERT INTO media_generation VALUES(?,?,?,?,?,?,?,?,?,?,?,?)", (identifier,"draw","reported","codex",None,"2026-11-22T00:00:00Z","chat","message",lineage,1,None,None))
            db.commit()
        backup.backup(self.data,self.store,self.keys,["attachments/attachments.sqlite3"],archive=archive,quiescent=True)
        destination=self.root / "new-media"
        (destination / "archive").mkdir(parents=True)
        (destination / "attachments").mkdir(mode=0o700)
        target=destination / "attachments/attachments.sqlite3"
        with closing(sqlite3.connect(target)) as db:
            db.execute(schema)
            db.commit()
        os.chmod(target,0o600)
        backup.restore_assets(destination,destination / "archive",self.store,self.keys,[identifier])
        with closing(sqlite3.connect(target)) as db:
            self.assertEqual(db.execute("SELECT prompt,lineage_id FROM media_generation").fetchone(),("draw",lineage))
            db.execute("UPDATE media_generation SET prompt='local edit'")
            db.commit()
        backup.restore_assets(destination,destination / "archive",self.store,self.keys,[identifier])
        with closing(sqlite3.connect(target)) as db:
            self.assertEqual(db.execute("SELECT prompt FROM media_generation").fetchone()[0],"local edit")

    def test_wal_snapshot_encrypts_and_restores(self):
        with closing(sqlite3.connect(self.data / "server.sqlite3")) as db:
            db.execute("PRAGMA journal_mode=WAL")
            db.execute("CREATE TABLE messages (body TEXT)")
            db.execute("INSERT INTO messages VALUES ('private example')")
            db.commit()
            (self.data / "vault.age").write_bytes(b"encrypted-vault-example")
            with self.assertRaisesRegex(ValueError, "vault_identity_unavailable"):
                backup.backup(self.data, self.store, self.keys, self.names)
            self.assertEqual(backup.backup(self.data, self.store, self.keys, self.names,
                                           vault_identity=b"synthetic-identity"), "uploaded")
            self.assertNotIn(b"private example", self.store.read(0))
            self.assertNotIn(b"encrypted-vault-example", self.store.read(0))
            self.assertNotIn(b"synthetic-identity", self.store.read(0))
        destination = self.root / "new"
        identities = []

        def writer(identity):
            identities.append(identity)
            return "created"

        self.assertEqual(backup.restore(destination, self.store, self.keys,
                                        vault_identity_writer=writer)["status"], "restored")
        self.assertEqual(identities, [b"synthetic-identity"])
        with closing(sqlite3.connect(destination / "server.sqlite3")) as db:
            self.assertEqual(db.execute("SELECT body FROM messages").fetchone()[0], "private example")
        self.assertEqual((destination / "vault.age").read_bytes(), b"encrypted-vault-example")

    def test_unchanged_and_seven_fixed_slots(self):
        self.write_db("one")
        self.assertEqual(backup.backup(self.data, self.store, self.keys, self.names), "uploaded")
        self.assertEqual(backup.backup(self.data, self.store, self.keys, self.names), "unchanged")
        for i in range(10):
            self.write_db(str(i))
            self.assertEqual(backup.backup(self.data, self.store, self.keys, self.names), "uploaded")
        self.assertEqual(set(self.store.objects), set(range(7)))
        self.assertEqual(backup.restore(self.root / "new", self.store, self.keys)["status"], "restored")
        with closing(sqlite3.connect(self.root / "new" / "server.sqlite3")) as db:
            self.assertEqual(db.execute("SELECT COUNT(*) FROM messages").fetchone()[0], 11)
        del self.store.objects[2]
        self.write_db("newer")
        self.assertEqual(backup.backup(self.data, self.store, self.keys, self.names), "uploaded")
        self.assertIn(2, self.store.objects)

    def test_existing_data_and_recovered_file_are_never_overwritten(self):
        self.write_db("remote")
        backup.backup(self.data, self.store, self.keys, self.names)
        existing = self.root / "existing"
        existing.mkdir()
        target = existing / "server.sqlite3"
        target.write_bytes(b"local data")
        self.assertEqual(backup.restore(existing, self.store, self.keys)["status"], "conflict")
        self.assertEqual(backup.restore(existing, self.store, self.keys)["status"], "conflict")
        self.assertEqual(target.read_bytes(), b"local data")
        self.assertTrue((existing / "server.sqlite3.recovered").exists())
        self.assertTrue((existing / "server.sqlite3.recovered-1").exists())

    def test_tampering_or_wrong_key_never_restores(self):
        self.write_db("private")
        backup.backup(self.data, self.store, self.keys, self.names)
        with self.assertRaises(InvalidTag):
            backup.restore(self.root / "wrong", self.store, FakeKeys())
        stamp, blob = self.store.objects[0]
        self.store.objects[0] = (stamp, blob.replace(b'"version":1', b'"version":2'))
        with self.assertRaises(ValueError):
            backup.restore(self.root / "tampered", self.store, self.keys)
        self.assertFalse((self.root / "tampered").exists())

    def test_archive_paths_cannot_escape_data_directory(self):
        with self.assertRaises(ValueError):
            backup.backup(self.data, self.store, self.keys, ["../private"])

    def test_multiple_sqlite_databases_are_restored(self):
        self.write_db("class note")
        with closing(sqlite3.connect(self.data / "class-student-private.sqlite3")) as db:
            db.execute("CREATE TABLE students (name TEXT)")
            db.execute("INSERT INTO students VALUES ('example')")
            db.commit()
        backup.backup(self.data, self.store, self.keys, backup.DATABASE_AND_VAULT)
        destination = self.root / "new"
        self.assertEqual(backup.restore(destination, self.store, self.keys)["status"], "restored")
        with closing(sqlite3.connect(destination / "class-student-private.sqlite3")) as db:
            self.assertEqual(db.execute("SELECT name FROM students").fetchone(), ("example",))

    def test_nested_server_credentials_and_attachment_database_are_encrypted_and_restored(self):
        attachment_dir = self.data / "attachments"
        attachment_dir.mkdir()
        with closing(sqlite3.connect(attachment_dir / "attachments.sqlite3")) as db:
            db.execute("CREATE TABLE assets (id TEXT)")
            db.execute("INSERT INTO assets VALUES ('fixture-asset')")
            db.commit()
        for name in ("saved-logins/vault.age", "provider-credentials/claude/vault.age", "provider-credentials/gemini/vault.age", "codex/auth.json"):
            path = self.data / name
            path.parent.mkdir(parents=True, exist_ok=True)
            path.write_bytes(b"synthetic-secret")
        backup.backup(self.data, self.store, self.keys, backup.DATABASE_AND_VAULT, vault_identity=b"synthetic-identity")
        self.assertNotIn(b"synthetic-secret", self.store.read(0))
        restored = self.root / "new-nested"
        result = backup.restore(restored, self.store, self.keys, vault_identity_writer=lambda _: "created")
        self.assertEqual(result["status"], "restored")
        self.assertEqual((restored / "codex/auth.json").read_bytes(), b"synthetic-secret")
        self.assertEqual((restored / "saved-logins/vault.age").read_bytes(), b"synthetic-secret")
        with closing(sqlite3.connect(restored / "attachments/attachments.sqlite3")) as db:
            self.assertEqual(db.execute("SELECT id FROM assets").fetchone(), ("fixture-asset",))

    @unittest.skipIf(os.name == "nt", "symlink setup requires a Linux test environment")
    def test_nested_parent_symlink_is_rejected(self):
        outside = self.root / "outside"
        outside.mkdir()
        (self.data / "saved-logins").symlink_to(outside, target_is_directory=True)
        with self.assertRaisesRegex(ValueError, "symlink"):
            backup.backup(self.data, self.store, self.keys, backup.DATABASE_AND_VAULT)

    def test_large_original_is_chunk_encrypted_verified_and_never_overwritten(self):
        archive = self.root / "archive"
        contents = b"synthetic-media-" + b"x" * backup.CHUNK_BYTES
        digest = hashlib.sha256(contents).hexdigest()
        shard = archive / "objects" / "sha256" / digest[:2]
        shard.mkdir(parents=True)
        original = shard / digest
        original.write_bytes(contents)
        descriptors = backup.backup_objects(archive, self.store, self.keys)
        self.assertEqual(descriptors[0]["chunks"], 2)
        old_chunks = dict(self.store.chunks)
        self.assertEqual(backup.backup_objects(archive, self.store, self.keys, descriptors), descriptors)
        self.assertEqual(self.store.chunks, old_chunks)
        self.assertEqual(original.read_bytes(), contents)
        self.assertTrue(all(b"synthetic-media-" not in chunk for chunk in self.store.chunks.values()))
        restored = self.root / "new-archive"
        backup.restore_objects(restored, self.store, self.keys, descriptors)
        target = restored / "objects" / "sha256" / digest[:2] / digest
        self.assertEqual(target.read_bytes(), contents)
        backup.restore_objects(restored, self.store, self.keys, descriptors)
        target.write_bytes(b"person data")
        with self.assertRaisesRegex(ValueError, "conflict"):
            backup.restore_objects(restored, self.store, self.keys, descriptors)
        self.assertEqual(target.read_bytes(), b"person data")

    def test_damaged_encrypted_chunk_does_not_install_object(self):
        archive = self.root / "archive"
        contents = b"private fixture"
        digest = hashlib.sha256(contents).hexdigest()
        shard = archive / "objects" / "sha256" / digest[:2]
        shard.mkdir(parents=True)
        (shard / digest).write_bytes(contents)
        descriptors = backup.backup_objects(archive, self.store, self.keys)
        key = (descriptors[0]["id"], 0)
        self.store.chunks[key] = self.store.chunks[key][:-1] + bytes([self.store.chunks[key][-1] ^ 1])
        restored = self.root / "damaged-archive"
        with self.assertRaisesRegex(ValueError, "integrity"):
            backup.restore_objects(restored, self.store, self.keys, descriptors)
        self.assertFalse((restored / "objects" / "sha256" / digest[:2] / digest).exists())

    def test_encrypted_backup_restores_database_and_original_together(self):
        self.write_db("fixture chat")
        archive = self.root / "archive"
        contents = b"fixture original media"
        digest = hashlib.sha256(contents).hexdigest()
        shard = archive / "objects" / "sha256" / digest[:2]
        shard.mkdir(parents=True)
        (shard / digest).write_bytes(contents)
        with self.assertRaisesRegex(ValueError, "quiescence_required"):
            backup.backup(self.data, self.store, self.keys, self.names, archive=archive)
        self.assertEqual(backup.backup(self.data, self.store, self.keys, self.names, archive=archive, quiescent=True), "uploaded")
        chunks = dict(self.store.chunks)
        self.assertEqual(backup.backup(self.data, self.store, self.keys, self.names, archive=archive, quiescent=True), "unchanged")
        self.assertEqual(self.store.chunks, chunks)
        with self.assertRaisesRegex(ValueError, "archive_destination_required"):
            backup.restore(self.root / "no-archive", self.store, self.keys)
        data = self.root / "restored-data"
        restored = self.root / "restored-archive"
        self.assertEqual(backup.restore(data, self.store, self.keys, archive=restored)["status"], "restored")
        self.assertEqual((restored / "objects" / "sha256" / digest[:2] / digest).read_bytes(), contents)
        with closing(sqlite3.connect(data / "server.sqlite3")) as db:
            self.assertEqual(db.execute("SELECT body FROM messages").fetchone(), ("fixture chat",))

    def test_google_adapters_use_fixed_object_names_and_kms(self):
        class Client:
            def __init__(self):
                self.calls = []

            def request(self, url, **kwargs):
                self.calls.append((url, kwargs))
                if url.endswith(":encrypt"):
                    return b'{"ciphertext":"ZW5jcnlwdGVk"}'
                if url.endswith(":decrypt"):
                    return b'{"plaintext":"a2V5"}'
                if kwargs.get("allow_missing"):
                    return None
                return b"contents"

        client = Client()
        keys = CloudKms(client, "projects/example/locations/global/keyRings/test/cryptoKeys/backup")
        self.assertEqual(keys.wrap(b"key"), b"encrypted")
        self.assertEqual(keys.unwrap(b"encrypted"), b"key")
        store = CloudStorage(client, "example-private-bucket")
        self.assertIsNone(store.modified(0))
        store.write(6, b"encrypted content")
        self.assertIn("slot-6.enc", client.calls[-1][0])
        self.assertTrue(all(call[1].get("method") != "DELETE" for call in client.calls))
        with self.assertRaises(ValueError):
            store.write(7, b"invalid")
        store.write_chunk("a" * 32, 0, b"encrypted chunk")
        self.assertIn("ifGenerationMatch=0", client.calls[-1][0])
        self.assertEqual(store.read_chunk("a" * 32, 0), b"contents")
        with self.assertRaises(ValueError):
            store.write_chunk("../escape", 0, b"invalid")

    def test_provisioning_rejects_another_installations_bucket(self):
        class Provider:
            install_id = "abcdef123456"
            project = "example-project"
            marker = "momso-install-" + install_id

            def run(self, *args, **kwargs):
                if args[:3] == ("storage", "buckets", "list"):
                    return [{"name": "momso-backup-" + self.install_id}]
                if args[:3] == ("storage", "buckets", "describe"):
                    return {"labels": {"momso-install-id": "someone-else"}}
                raise AssertionError("unowned resource must not be modified")

        with self.assertRaises(InstallError):
            GoogleBackupProvisioner(Provider()).provision()

    def test_provisioning_binds_only_dedicated_account(self):
        class Provider:
            install_id = "abcdef123456"
            project = "example-project"
            marker = "momso-install-" + install_id

            def __init__(self):
                self.calls = []

            def run(self, *args, **kwargs):
                self.calls.append(args)
                return [] if kwargs.get("parse") else b""

        provider = Provider()
        result = GoogleBackupProvisioner(provider).provision()
        self.assertEqual(set(result["bucket_permissions"]), {
            "storage.objects.get", "storage.objects.create", "storage.objects.delete"
        })
        bindings = [call for call in provider.calls if "add-iam-policy-binding" in call]
        self.assertEqual(len(bindings), 2)
        self.assertTrue(all("--member=serviceAccount:" + result["service_account"] in call for call in bindings))
        self.assertTrue(any("--public-access-prevention" in call for call in provider.calls))
        self.assertFalse(any("delete" == part for call in provider.calls for part in call))

    def test_destroy_preserves_backup_unless_explicit_and_owned(self):
        class Provider:
            install_id = "abcdef123456"
            project = "example-project"
            marker = "momso-install-" + install_id

            def __init__(self):
                self.calls = []
                self.owner = self.install_id

            def run(self, *args, **kwargs):
                self.calls.append(args)
                if args[:3] == ("storage", "buckets", "list"):
                    return [{"name": "momso-backup-" + self.install_id}]
                if args[:3] == ("storage", "buckets", "describe"):
                    return {"labels": {"momso-install-id": self.owner}}
                return b""

        provider = Provider()
        provisioner = GoogleBackupProvisioner(provider)
        self.assertEqual(provisioner.destroy(), "backup_preserved")
        self.assertEqual(provider.calls, [])
        provider.owner = "different-install"
        with self.assertRaises(InstallError):
            provisioner.destroy(delete_backup=True)
        self.assertFalse(any(call[:2] == ("storage", "rm") for call in provider.calls))
        provider.owner = provider.install_id
        self.assertEqual(provisioner.destroy(delete_backup=True), "backup_deleted")

    def test_host_credential_rewrap_never_overwrites_or_writes_plaintext(self):
        calls = []

        def fake_run(command, **kwargs):
            calls.append((command, kwargs.get("input")))
            return SimpleNamespace(returncode=0, stdout=b"fake-host-encrypted")

        path = self.root / "credentials" / "momso-vault-identity"
        self.assertEqual(rewrap_for_host(b"synthetic-private-identity", path, run=fake_run), "created")
        self.assertEqual(rewrap_for_host(b"different-identity", path, run=fake_run), "conflict")
        self.assertEqual(path.read_bytes(), b"fake-host-encrypted")
        self.assertNotIn("synthetic-private-identity", " ".join(calls[0][0]))

        def retry_run(command, **kwargs):
            self.assertEqual(command[1], "decrypt")
            return SimpleNamespace(returncode=0, stdout=b"synthetic-private-identity")

        self.assertEqual(rewrap_for_host(b"synthetic-private-identity", path, run=retry_run), "already_present")


if __name__ == "__main__":
    unittest.main()
