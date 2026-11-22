import os
import errno
import hashlib
import io
import json
import sqlite3
import tempfile
import threading
import unittest
from unittest import mock
import urllib.parse
from contextlib import closing
from pathlib import Path
from types import SimpleNamespace

from cryptography.hazmat.primitives.ciphers.aead import AESGCM
from cryptography.exceptions import InvalidTag

import momso_anywhere_backup as backup
from momso_anywhere_backup_google import CloudKms, CloudStorage, GoogleBackupError
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
        self.ledger = None
        self.ledger_generation = None
        self.retained_slots = {}
        self.retained_chunks = {}

    def read_purge_ledger(self):
        return self.ledger

    def read_purge_ledger_versioned(self):
        return (self.ledger_generation, self.ledger) if self.ledger is not None else None

    def write_purge_ledger_if_generation(self, generation, value):
        if generation != (self.ledger_generation if self.ledger is not None else "0"):
            raise ValueError("generation changed")
        self.clock += 1
        self.ledger_generation = str(self.clock)
        self.ledger = value

    def modified(self, slot):
        return self.objects[slot][0] if slot in self.objects else None

    def read(self, slot):
        return self.objects[slot][1]

    def write(self, slot, content):
        self.clock += 1
        self.objects[slot] = (self.clock, content)

    def read_versioned(self, slot):
        return (str(self.objects[slot][0]), self.objects[slot][1]) if slot in self.objects else None

    def replace_if_generation(self, slot, generation, content):
        if slot not in self.objects or str(self.objects[slot][0]) != generation:
            raise ValueError("generation changed")
        self.write(slot, content)

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

    def list_slot_generations(self, slot, *, soft_deleted=False):
        if soft_deleted:
            return self.retained_slots.get(slot, [])
        generation = self.read_versioned(slot)
        return [{"generation": generation[0]}] if generation else []

    def list_chunk_generations(self, identifier, index, *, soft_deleted=False):
        key = (identifier, index)
        if soft_deleted:
            return self.retained_chunks.get(key, [])
        return [{"generation": "1"}] if key in self.chunks else []

    def delete_chunk_generation(self, identifier, index, generation):
        if generation != "1": raise ValueError("generation changed")
        self.chunks.pop((identifier, index), None)


class FileStore:
    """Synthetic remote shared across a killed worker and its replacement."""
    def __init__(self, root):
        self.root = Path(root)
        self.root.mkdir(parents=True, exist_ok=True)
    def _path(self, name): return self.root / name
    def _generation(self, name):
        path = self._path(name + ".generation")
        return path.read_text() if path.exists() else None
    def _write(self, name, value):
        counter = self._path("clock")
        number = int(counter.read_text()) + 1 if counter.exists() else 1
        counter.write_text(str(number))
        self._path(name).write_bytes(value)
        self._path(name + ".generation").write_text(str(number))
    def modified(self, slot):
        value = self._generation("slot-" + str(slot))
        return int(value) if value is not None else None
    def read(self, slot): return self._path("slot-" + str(slot)).read_bytes()
    def write(self, slot, value): self._write("slot-" + str(slot), value)
    def read_versioned(self, slot):
        version = self._generation("slot-" + str(slot))
        return (version, self.read(slot)) if version else None
    def replace_if_generation(self, slot, generation, value):
        if self._generation("slot-" + str(slot)) != generation: raise ValueError("generation changed")
        self.write(slot, value)
    def write_chunk(self, identifier, index, value):
        path = self._path("chunk-" + identifier + "-" + str(index))
        with path.open("xb") as output: output.write(value)
    def read_chunk(self, identifier, index): return self._path("chunk-" + identifier + "-" + str(index)).read_bytes()
    def chunk_matches(self, identifier, index, size, md5_hex):
        path = self._path("chunk-" + identifier + "-" + str(index))
        return path.exists() and path.stat().st_size == size and hashlib.md5(path.read_bytes(), usedforsecurity=False).hexdigest() == md5_hex
    def read_purge_ledger(self):
        path = self._path("ledger")
        return path.read_bytes() if path.exists() else None
    def read_purge_ledger_versioned(self):
        value = self.read_purge_ledger()
        return (self._generation("ledger"), value) if value is not None else None
    def write_purge_ledger_if_generation(self, generation, value):
        if generation != (self._generation("ledger") or "0"): raise ValueError("generation changed")
        self._write("ledger", value)


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

    @unittest.skipUnless(os.name == "posix", "backup agent requires POSIX locking")
    def test_snapshot_signal_is_opt_in_for_older_servers(self):
        import momso_anywhere_backup_agent as agent
        self.write_db("example")
        env = {"MOMSO_SERVER_DATA_DIR": str(self.data), "MOMSO_BACKUP_BUCKET": "fixture",
               "MOMSO_BACKUP_KMS_KEY": "fixture"}
        stale = self.data / ".backup-runtime/snapshot-stale"
        stale.mkdir(parents=True)
        (stale / "old").write_bytes(b"fixture")
        with mock.patch.dict(os.environ, env, clear=True), mock.patch.object(agent, "GoogleClient"), \
             mock.patch.object(agent, "CloudStorage", return_value=self.store), \
             mock.patch.object(agent, "CloudKms", return_value=self.keys), \
             mock.patch("sys.argv", ["agent", "backup"]):
            os.environ["MOMSO_BACKUP_QUIESCENT"] = "1"
            with mock.patch("sys.stdout", new_callable=io.StringIO) as output:
                agent.main()
                self.assertEqual(output.getvalue(), "momso-backup-v3:uploaded\n")
                self.assertFalse(stale.exists())
            os.environ["MOMSO_BACKUP_SNAPSHOT_SIGNAL"] = "1"
            with mock.patch("sys.stdout", new_callable=io.StringIO) as output:
                agent.main()
                self.assertTrue(output.getvalue().startswith("momso-backup-v3:snapshot-ready\n"))

    @unittest.skipUnless(os.name == "posix", "backup agent requires POSIX locking")
    def test_agent_records_purge_under_backup_lock_and_retries(self):
        import momso_anywhere_backup_agent as agent
        digest = hashlib.sha256(b"sample").hexdigest()
        env = {"MOMSO_SERVER_DATA_DIR": str(self.data),
               "MOMSO_BACKUP_BUCKET": "fake", "MOMSO_BACKUP_KMS_KEY": "fake"}
        with mock.patch.dict(os.environ, env), mock.patch.object(agent, "GoogleClient"), \
             mock.patch.object(agent, "CloudStorage", return_value=self.store), \
             mock.patch.object(agent, "CloudKms", return_value=self.keys), \
             mock.patch("sys.argv", ["agent", "record-purge"]):
            for _ in range(2):
                with mock.patch("sys.stdin", io.StringIO(json.dumps({"digest": digest}))), \
                     mock.patch("sys.stdout", new_callable=io.StringIO) as output:
                    agent.main()
                    self.assertEqual(output.getvalue(), "momso-backup-v3:purge-recorded\n")
            self.assertEqual(backup.read_purged_ledger(self.store, self.keys), {digest})
            with mock.patch("sys.stdin", io.StringIO('{"digest":"invalid"}')):
                with self.assertRaises(ValueError):
                    agent.main()

    @unittest.skipUnless(os.name == "posix", "backup agent requires POSIX locking")
    def test_agent_rejects_purge_when_backup_staging_shares_content(self):
        import momso_anywhere_backup_agent as agent
        content = b"shared staged upload"
        digest = hashlib.sha256(content).hexdigest()
        archive = self.data / "archive"
        stage = self.data / "attachments" / "upload-staging" / "12345678-1234-1234-1234-123456789abc"
        stage.parent.mkdir(parents=True)
        stage.write_bytes(content)
        backup.backup(self.data, self.store, self.keys, [], archive=archive, quiescent=True)
        env = {"MOMSO_SERVER_DATA_DIR": str(self.data), "MOMSO_BACKUP_BUCKET": "fake", "MOMSO_BACKUP_KMS_KEY": "fake"}
        with mock.patch.dict(os.environ, env), mock.patch.object(agent, "GoogleClient"), \
             mock.patch.object(agent, "CloudStorage", return_value=self.store), \
             mock.patch.object(agent, "CloudKms", return_value=self.keys), \
             mock.patch("sys.argv", ["agent", "record-purge"]), \
             mock.patch("sys.stdin", io.StringIO(json.dumps({"digest": digest}))):
            with self.assertRaisesRegex(ValueError, "shared"):
                agent.main()
        self.assertEqual(backup.read_purged_ledger(self.store, self.keys), set())

    @unittest.skipUnless(os.name == "posix", "backup agent requires POSIX locking")
    def test_agent_purge_waits_for_backup_restore_lock(self):
        import fcntl
        import momso_anywhere_backup_agent as agent
        digest = hashlib.sha256(b"locked fixture").hexdigest()
        runtime = self.data / ".backup-runtime"
        runtime.mkdir()
        started, done, errors = threading.Event(), threading.Event(), []
        def run():
            try:
                started.set()
                agent.main()
            except Exception as error:
                errors.append(error)
            finally:
                done.set()
        env = {"MOMSO_SERVER_DATA_DIR": str(self.data), "MOMSO_BACKUP_BUCKET": "fake", "MOMSO_BACKUP_KMS_KEY": "fake"}
        with open(runtime / "lock", "a+b") as lock, mock.patch.dict(os.environ, env), \
             mock.patch.object(agent, "GoogleClient"), mock.patch.object(agent, "CloudStorage", return_value=self.store), \
             mock.patch.object(agent, "CloudKms", return_value=self.keys), \
             mock.patch("sys.argv", ["agent", "record-purge"]), \
             mock.patch("sys.stdin", io.StringIO(json.dumps({"digest": digest}))), \
             mock.patch("sys.stdout", new_callable=io.StringIO):
            fcntl.flock(lock, fcntl.LOCK_EX)
            worker = threading.Thread(target=run)
            worker.start()
            try:
                self.assertTrue(started.wait(5))
                self.assertFalse(done.wait(0.2), "purge bypassed the backup lock")
            finally:
                fcntl.flock(lock, fcntl.LOCK_UN)
                worker.join(timeout=5)
            self.assertFalse(worker.is_alive())
            self.assertEqual(errors, [])
        self.assertEqual(backup.read_purged_ledger(self.store, self.keys), {digest})

    def write_db(self, value):
        with closing(sqlite3.connect(self.data / "server.sqlite3")) as db:
            db.execute("CREATE TABLE IF NOT EXISTS messages (body TEXT)")
            db.execute("INSERT INTO messages VALUES (?)", (value,))
            db.commit()

    def test_purge_plan_inspects_all_slots_without_deleting_shared_content(self):
        self.write_db("one")
        archive = self.data / "archive"
        content = b"synthetic-original"
        digest = hashlib.sha256(content).hexdigest()
        original = archive / "objects" / "sha256" / digest[:2] / digest
        original.parent.mkdir(parents=True)
        original.write_bytes(content)
        backup.backup(self.data, self.store, self.keys, ["server.sqlite3"], archive=archive, quiescent=True)
        self.write_db("two")
        backup.backup(self.data, self.store, self.keys, ["server.sqlite3"], archive=archive, quiescent=True)
        before = (dict(self.store.objects), dict(self.store.chunks))
        plan = backup.plan_original_purge(self.store, self.keys, digest)
        self.assertEqual(plan["slots"], [0, 1])
        self.assertTrue(plan["chunks"])
        self.assertEqual((self.store.objects, self.store.chunks), before)
        slot = backup._unpack(backup._decrypt(self.store.read(0), self.keys))
        rewritten = backup.without_original(slot, digest)
        rewritten_manifest = json.loads(rewritten[".momso-v3.json"])
        self.assertNotIn(digest, rewritten_manifest["originals"])
        self.assertNotIn(digest, [item["sha256"] for item in rewritten_manifest["descriptors"]])
        self.assertIn("server.sqlite3", rewritten_manifest["files"])
        self.assertEqual(backup._unpack(backup._pack(rewritten)), rewritten)
        self.assertTrue(backup.replace_slot_without_original(self.store, self.keys, 1, digest))
        self.assertFalse(backup.replace_slot_without_original(self.store, self.keys, 1, digest))
        self.assertEqual(self.store.chunks, before[1])
        generation, old_value = self.store.read_versioned(1)
        self.store.write(1, old_value)
        with self.assertRaisesRegex(ValueError, "generation changed"):
            self.store.replace_if_generation(1, generation, b"unsafe overwrite")
        manifest = json.loads(slot[".momso-v3.json"])
        manifest["files"]["server.sqlite3"] = digest
        slot[".momso-v3.json"] = json.dumps(manifest).encode()
        self.store.write(0, backup._encrypt(backup._pack(slot), self.keys))
        with self.assertRaisesRegex(ValueError, "shared"):
            backup.plan_original_purge(self.store, self.keys, digest)
        with self.assertRaisesRegex(ValueError, "shared"):
            backup.without_original(slot, digest)
        self.assertEqual(self.store.chunks, before[1])

    def test_slot_rewrite_resumes_after_crash_and_old_slots_restore_other_files(self):
        self.write_db("preserve database")
        archive = self.data / "archive"
        originals = {}
        for content in (b"delete fixture", b"keep fixture"):
            digest = hashlib.sha256(content).hexdigest()
            path = archive / "objects" / "sha256" / digest[:2] / digest
            path.parent.mkdir(parents=True, exist_ok=True)
            path.write_bytes(content)
            originals[content] = (digest, path)
        backup.backup(self.data, self.store, self.keys, ["server.sqlite3"], archive=archive, quiescent=True)
        self.write_db("second generation")
        backup.backup(self.data, self.store, self.keys, ["server.sqlite3"], archive=archive, quiescent=True)
        digest = originals[b"delete fixture"][0]
        chunks_before = dict(self.store.chunks)
        journal_dir = self.root / "journal"
        journal_dir.mkdir()
        def interrupted(journal):
            if backup.read_purge_journal(journal_dir) is not None:
                raise RuntimeError("simulated crash")
            backup.write_purge_journal(journal_dir, journal)
        with self.assertRaisesRegex(RuntimeError, "simulated crash"):
            backup.resume_original_slot_rewrite(self.store, self.keys, digest, None, interrupted)
        self.assertEqual(self.store.chunks, chunks_before)
        self.assertTrue(originals[b"delete fixture"][1].exists())
        saved = backup.read_purge_journal(journal_dir)
        self.assertEqual(saved["completed"], [])
        journal = backup.resume_original_slot_rewrite(
            self.store, self.keys, digest, saved,
            lambda value: backup.write_purge_journal(journal_dir, value))
        self.assertEqual(journal["completed"], [0, 1])
        self.assertEqual(backup.read_purge_journal(journal_dir)["completed"], [0, 1])
        self.assertEqual(self.store.chunks, chunks_before)
        self.assertFalse(backup.plan_original_purge(self.store, self.keys, digest)["slots"])
        for slot, (_, contents) in self.store.objects.items():
            remote = FakeStore()
            remote.write(0, contents)
            remote.chunks = dict(self.store.chunks)
            destination = self.root / f"restored-{slot}"
            result = backup.restore(destination, remote, self.keys, names=["server.sqlite3"], archive=destination / "archive")
            self.assertEqual(result["status"], "restored")
            self.assertTrue((destination / "server.sqlite3").is_file())
            keep = originals[b"keep fixture"][0]
            self.assertEqual((destination / "archive/objects/sha256" / keep[:2] / keep).read_bytes(), b"keep fixture")
            self.assertFalse((destination / "archive/objects/sha256" / digest[:2] / digest).exists())

    def test_purge_never_changes_backup_if_journal_cannot_be_saved(self):
        self.write_db("fixture")
        archive = self.data / "archive"
        content = b"source to preserve"
        digest = hashlib.sha256(content).hexdigest()
        source = archive / "objects" / "sha256" / digest[:2] / digest
        source.parent.mkdir(parents=True)
        source.write_bytes(content)
        backup.backup(self.data, self.store, self.keys, ["server.sqlite3"], archive=archive, quiescent=True)
        before = (dict(self.store.objects), dict(self.store.chunks))
        def cannot_persist(_journal):
            raise OSError("disk full")
        with self.assertRaises(OSError):
            backup.resume_original_slot_rewrite(self.store, self.keys, digest, None, cannot_persist)
        self.assertEqual((self.store.objects, self.store.chunks), before)
        self.assertEqual(source.read_bytes(), content)
        with self.assertRaisesRegex(ValueError, "invalid purge journal"):
            backup.resume_original_slot_rewrite(self.store, self.keys, digest, {"version": 1}, lambda _: None)
        old_plan = backup._new_purge_journal(digest, backup.plan_original_purge(self.store, self.keys, digest))
        old_plan["version"] = 1
        old_plan.pop("planHash")
        directory = self.root / "unverified-old-journal"
        directory.mkdir()
        backup.write_purge_journal(directory, old_plan)
        with self.assertRaisesRegex(ValueError, "purge journal requires review"):
            backup.prepare_original_backup_purge(self.store, self.keys, digest, directory)
        self.assertIsNone(self.store.ledger)
        self.assertEqual((self.store.objects, self.store.chunks), before)

    def test_new_backup_after_purge_plan_fails_before_publishing_deletion(self):
        self.write_db("fixture")
        archive = self.data / "archive"
        content = b"do not delete concurrently"
        digest = hashlib.sha256(content).hexdigest()
        original = archive / "objects" / "sha256" / digest[:2] / digest
        original.parent.mkdir(parents=True)
        original.write_bytes(content)
        backup.backup(self.data, self.store, self.keys, ["server.sqlite3"], archive=archive, quiescent=True)
        plan = backup.plan_original_purge(self.store, self.keys, digest)
        directory = self.root / "concurrent-journal"
        directory.mkdir()
        backup.write_purge_journal(directory, backup._new_purge_journal(digest, plan))
        self.write_db("concurrent edit")
        backup.backup(self.data, self.store, self.keys, ["server.sqlite3"], archive=archive, quiescent=True)
        before = (dict(self.store.objects), dict(self.store.chunks))
        with self.assertRaisesRegex(ValueError, "purge fence violated"):
            backup.prepare_original_backup_purge(self.store, self.keys, digest, directory)
        self.assertIsNone(self.store.ledger)
        self.assertEqual((self.store.objects, self.store.chunks), before)
        self.assertEqual(original.read_bytes(), content)

    def test_chunk_cleanup_preserves_other_data_and_reports_retention(self):
        self.write_db("preserve fixture")
        archive = self.data / "archive"
        digests = {}
        for content in (b"delete content", b"keep content"):
            digest = hashlib.sha256(content).hexdigest()
            path = archive / "objects/sha256" / digest[:2] / digest
            path.parent.mkdir(parents=True, exist_ok=True)
            path.write_bytes(content)
            digests[content] = (digest, path)
        backup.backup(self.data, self.store, self.keys, ["server.sqlite3"], archive=archive, quiescent=True)
        directory = self.root / "cleanup-journal"
        directory.mkdir()
        digest = digests[b"delete content"][0]
        journal = backup.prepare_original_backup_purge(self.store, self.keys, digest, directory)
        planned = {tuple(value) for value in journal["chunks"]}
        before = dict(self.store.chunks)
        corrupt = json.loads(json.dumps(journal))
        corrupt["chunks"] = []
        with self.assertRaisesRegex(ValueError, "invalid purge journal"):
            backup.erase_prepared_backup_chunks(self.store, self.keys, digest, corrupt)
        self.assertEqual(self.store.chunks, before)
        real_list = self.store.list_slot_generations
        self.store.list_slot_generations = lambda _slot, *, soft_deleted=False: []
        with self.assertRaisesRegex(ValueError, "incomplete backup generation listing"):
            backup.erase_prepared_backup_chunks(self.store, self.keys, digest, journal)
        self.assertEqual(self.store.chunks, before)
        self.store.list_slot_generations = real_list
        slot_before = self.store.read(0)
        slot = backup._unpack(backup._decrypt(slot_before, self.keys))
        manifest = json.loads(slot[".momso-v3.json"])
        kept = next(item for item in manifest["descriptors"] if item["sha256"] == digests[b"keep content"][0])
        kept["id"] = next(iter(planned))[0]
        slot[".momso-v3.json"] = json.dumps(manifest).encode()
        self.store.write(0, backup._encrypt(backup._pack(slot), self.keys))
        with self.assertRaisesRegex(ValueError, "purge chunk still referenced"):
            backup.erase_prepared_backup_chunks(self.store, self.keys, digest, journal)
        self.assertEqual(self.store.chunks, before)
        self.store.write(0, slot_before)
        self.store.retained_slots[0] = [{"generation":"old", "hardDeleteTime":"2026-12-01T00:00:00Z"}]
        self.store.retained_slots[1] = [{"generation":"older", "hardDeleteTime":"2026-12-02T09:00:00+09:00"}]
        pending = backup.erase_prepared_backup_chunks(self.store, self.keys, digest, journal)
        self.assertEqual(pending["reason"], "historical_backup")
        self.assertEqual(pending["retentionUntil"], "2026-12-02T00:00:00Z")
        self.assertEqual(self.store.chunks, before)
        self.store.retained_slots.clear()
        real_delete = self.store.delete_chunk_generation
        def retain(identifier, index, generation):
            real_delete(identifier, index, generation)
            self.store.retained_chunks[(identifier, index)] = [{"generation":generation,"hardDeleteTime":"2026-12-01T00:00:00Z"}]
        self.store.delete_chunk_generation = retain
        result = backup.erase_prepared_backup_chunks(self.store, self.keys, digest, journal)
        self.assertEqual(result, {"state":"retained", "reason":"cloud_retention", "retentionUntil":"2026-12-01T00:00:00Z"})
        self.assertFalse(planned.intersection(self.store.chunks))
        self.assertEqual({key: value for key, value in before.items() if key not in planned}, self.store.chunks)
        self.assertEqual(digests[b"keep content"][1].read_bytes(), b"keep content")
        self.assertEqual(digests[b"delete content"][1].read_bytes(), b"delete content")
        self.store.retained_chunks.clear()
        self.assertEqual(backup.erase_prepared_backup_chunks(self.store, self.keys, digest, journal)["state"], "deleted")

    def test_ledger_and_slot_cleanup_resume_after_interrupted_remote_write(self):
        self.write_db("keep this database")
        archive = self.data / "archive"
        content = b"fixture to delete"
        digest = hashlib.sha256(content).hexdigest()
        original = archive / "objects" / "sha256" / digest[:2] / digest
        original.parent.mkdir(parents=True)
        original.write_bytes(content)
        backup.backup(self.data, self.store, self.keys, ["server.sqlite3"], archive=archive, quiescent=True)
        self.write_db("later entry")
        backup.backup(self.data, self.store, self.keys, ["server.sqlite3"], archive=archive, quiescent=True)
        journal_dir = self.root / "purge-journal"
        journal_dir.mkdir()
        real_replace = self.store.replace_if_generation
        def disconnected(*_args):
            raise OSError("fake network interruption")
        self.store.replace_if_generation = disconnected
        with self.assertRaisesRegex(OSError, "fake network interruption"):
            backup.prepare_original_backup_purge(self.store, self.keys, digest, journal_dir)
        self.assertIn(digest, backup.read_purged_ledger(self.store, self.keys))
        self.assertEqual(backup.read_purge_journal(journal_dir)["completed"], [])
        self.assertEqual(original.read_bytes(), content)
        destination = self.root / "new-vm-during-retry"
        self.assertEqual(backup.restore(destination, self.store, self.keys, names=["server.sqlite3"], archive=destination / "archive")["status"], "restored")
        self.assertFalse((destination / "archive/objects/sha256" / digest[:2] / digest).exists())
        self.store.replace_if_generation = real_replace
        journal = backup.prepare_original_backup_purge(self.store, self.keys, digest, journal_dir)
        self.assertEqual(journal["completed"], [0, 1])
        self.assertEqual(backup.plan_original_purge(self.store, self.keys, digest)["slots"], [])
        self.assertEqual(original.read_bytes(), content)

    @unittest.skipUnless(hasattr(os, "fork"), "requires isolated Linux process")
    def test_killed_purge_worker_resumes_from_fsynced_journal(self):
        self.write_db("document and chat fixture")
        archive = self.data / "archive"
        content = b"synthetic deleted original"
        digest = hashlib.sha256(content).hexdigest()
        original = archive / "objects" / "sha256" / digest[:2] / digest
        original.parent.mkdir(parents=True)
        original.write_bytes(content)
        remote = FileStore(self.root / "fake-remote")
        backup.backup(self.data, remote, self.keys, ["server.sqlite3"], archive=archive, quiescent=True)
        self.write_db("second snapshot")
        backup.backup(self.data, remote, self.keys, ["server.sqlite3"], archive=archive, quiescent=True)
        journal_dir = self.root / "durable-journal"
        journal_dir.mkdir()
        child = os.fork()
        if child == 0:
            real_replace = remote.replace_if_generation
            def kill_after_slot(*args):
                real_replace(*args)
                os._exit(67)
            remote.replace_if_generation = kill_after_slot
            backup.prepare_original_backup_purge(remote, self.keys, digest, journal_dir)
            os._exit(0)
        _, status = os.waitpid(child, 0)
        self.assertTrue(os.WIFEXITED(status))
        self.assertEqual(os.WEXITSTATUS(status), 67)
        self.assertEqual(backup.read_purge_journal(journal_dir)["completed"], [])
        self.assertIn(digest, backup.read_purged_ledger(remote, self.keys))
        restored = self.root / "new-vm"
        self.assertEqual(backup.restore(restored, remote, self.keys, names=["server.sqlite3"], archive=restored / "archive")["status"], "restored")
        self.assertFalse((restored / "archive/objects/sha256" / digest[:2] / digest).exists())
        journal = backup.prepare_original_backup_purge(remote, self.keys, digest, journal_dir)
        self.assertEqual(journal["completed"], [0, 1])
        self.assertEqual(original.read_bytes(), content)

    def test_old_backup_restores_deleted_asset_as_marker_not_bytes(self):
        archive = self.data / "archive"
        data = b"deleted original fixture"
        digest = hashlib.sha256(data).hexdigest()
        original = archive / "objects" / "sha256" / digest[:2] / digest
        original.parent.mkdir(parents=True)
        original.write_bytes(data)
        attachment_dir = self.data / "attachments"
        attachment_dir.mkdir()
        asset_id = "11111111-1111-4111-8111-111111111111"
        with closing(sqlite3.connect(attachment_dir / "attachments.sqlite3")) as database:
            database.execute("CREATE TABLE assets(id TEXT PRIMARY KEY,hash TEXT UNIQUE NOT NULL,bytes INTEGER NOT NULL,name TEXT NOT NULL,mime TEXT NOT NULL,kind TEXT NOT NULL)")
            database.execute("INSERT INTO assets VALUES(?,?,?,?,?,?)", (asset_id, digest, len(data), "fixture.txt", "text/plain", "document"))
            database.commit()
        backup.backup(self.data, self.store, self.keys, ["attachments/attachments.sqlite3"], archive=archive, quiescent=True)
        old_encrypted_slot = self.store.read(0)
        backup.resume_original_slot_rewrite(self.store, self.keys, digest, None, lambda _: None)
        target = self.root / "restored-purged"
        self.assertEqual(backup.restore(target, self.store, self.keys, names=["attachments/attachments.sqlite3"], archive=target / "archive")["status"], "restored")
        with closing(sqlite3.connect(target / "attachments/attachments.sqlite3")) as database:
            self.assertEqual(database.execute("SELECT asset_id FROM asset_purged").fetchone(), (asset_id,))
            self.assertEqual(database.execute("SELECT name FROM assets WHERE id=?", (asset_id,)).fetchone(), ("fixture.txt",))
        self.assertFalse((target / "archive/objects/sha256" / digest[:2] / digest).exists())
        old_remote = FakeStore()
        old_remote.write(0, old_encrypted_slot)
        old_remote.chunks = dict(self.store.chunks)
        self.assertTrue(backup.record_purged_original(old_remote, self.keys, digest))
        self.assertFalse(backup.record_purged_original(old_remote, self.keys, digest))
        previous = old_remote.read_purge_ledger_versioned()
        backup.record_purged_original(old_remote, self.keys, "f" * 64)
        with self.assertRaisesRegex(ValueError, "generation changed"):
            old_remote.write_purge_ledger_if_generation(previous[0], previous[1])
        older = self.root / "older-generation"
        self.assertEqual(backup.restore(older, old_remote, self.keys, names=["attachments/attachments.sqlite3"], archive=older / "archive")["status"], "restored")
        with closing(sqlite3.connect(older / "attachments/attachments.sqlite3")) as database:
            self.assertEqual(database.execute("SELECT asset_id FROM asset_purged").fetchone(), (asset_id,))
        self.assertFalse((older / "archive/objects/sha256" / digest[:2] / digest).exists())
        with self.assertRaisesRegex(ValueError, "purged original remains"):
            backup.backup(self.data, old_remote, self.keys, ["attachments/attachments.sqlite3"], archive=archive, quiescent=True)
        self.assertEqual(original.read_bytes(), data)
        old_remote.ledger = backup._encrypt(b'{"version":1,"digests":["not-a-hash"]}', self.keys)
        before = (dict(old_remote.objects), dict(old_remote.chunks))
        with self.assertRaisesRegex(ValueError, "invalid purge ledger"):
            backup.restore(self.root / "invalid-ledger", old_remote, self.keys, names=["attachments/attachments.sqlite3"], archive=self.root / "invalid-ledger/archive")
        with self.assertRaisesRegex(ValueError, "invalid purge ledger"):
            backup.backup(self.data, old_remote, self.keys, ["attachments/attachments.sqlite3"], archive=archive, quiescent=True)
        self.assertEqual((old_remote.objects, old_remote.chunks), before)

    @unittest.skipIf(os.name == "nt", "requires Linux attachment lock")
    def test_selective_old_chat_restore_keeps_deleted_attachment_name_only(self):
        identifier = "11111111-1111-4111-8111-111111111111"
        content = b"old attachment"
        digest = hashlib.sha256(content).hexdigest()
        archive = self.data / "archive"
        original = archive / "objects/sha256" / digest[:2] / digest
        original.parent.mkdir(parents=True)
        original.write_bytes(content)
        database = self.data / "attachments/attachments.sqlite3"
        database.parent.mkdir()
        schema = "CREATE TABLE assets(id TEXT PRIMARY KEY,hash TEXT UNIQUE NOT NULL,bytes INTEGER NOT NULL,name TEXT NOT NULL,mime TEXT NOT NULL,kind TEXT NOT NULL)"
        with closing(sqlite3.connect(database)) as db:
            db.execute(schema)
            db.execute("INSERT INTO assets VALUES(?,?,?,?,?,?)", (identifier, digest, len(content), "lesson.txt", "text/plain", "document"))
            db.commit()
        backup.backup(self.data, self.store, self.keys, ["attachments/attachments.sqlite3"], archive=archive, quiescent=True)
        backup.record_purged_original(self.store, self.keys, digest)
        destination = self.root / "new-vm-selective"
        (destination / "archive").mkdir(parents=True)
        (destination / "attachments").mkdir(mode=0o700)
        target = destination / "attachments/attachments.sqlite3"
        with closing(sqlite3.connect(target)) as db:
            db.execute(schema)
            db.commit()
        os.chmod(target, 0o600)
        for _ in range(2):
            backup.restore_assets(destination, destination / "archive", self.store, self.keys, [identifier])
        with closing(sqlite3.connect(target)) as db:
            self.assertEqual(db.execute("SELECT name FROM assets WHERE id=?", (identifier,)).fetchone(), ("lesson.txt",))
            self.assertEqual(db.execute("SELECT asset_id FROM asset_purged").fetchone(), (identifier,))
        self.assertFalse((destination / "archive/objects/sha256" / digest[:2] / digest).exists())

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
            db.execute("CREATE TABLE asset_dates(asset_id TEXT PRIMARY KEY,created_at TEXT NOT NULL,source TEXT NOT NULL)")
            db.execute("INSERT INTO assets VALUES(?,?,?,?,?,?)", (identifier, digest, len(content), "example.txt", "text/plain", "document"))
            db.execute("INSERT INTO asset_trash VALUES(?,?)", (identifier, "2026-11-22T00:00:00Z"))
            db.execute("INSERT INTO asset_dates VALUES(?,?,?)", (identifier, "2026-11-21T12:00:00Z", "registered"))
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
        if os.name != "nt":
            import fcntl
            import threading
            import time
            held = os.open(destination / "attachments/attachments.lock", os.O_RDWR | os.O_CREAT, 0o600)
            fcntl.flock(held, fcntl.LOCK_EX)
            def release_lock():
                time.sleep(0.15)
                fcntl.flock(held, fcntl.LOCK_UN)
            releasing = threading.Thread(target=release_lock)
            releasing.start()
            try:
                backup.restore_assets(destination, destination / "archive", self.store, self.keys, [identifier])
            finally:
                releasing.join()
                os.close(held)
        for _ in range(2):
            backup.restore_assets(destination, destination / "archive", self.store, self.keys, [identifier])
        with closing(sqlite3.connect(target_db)) as db:
            self.assertEqual(db.execute("SELECT id,hash FROM assets").fetchall(), [(identifier, digest)])
            self.assertEqual(db.execute("SELECT asset_id FROM asset_trash").fetchall(), [(identifier,)])
            self.assertEqual(db.execute("SELECT created_at,source FROM asset_dates").fetchone(), ("2026-11-21T12:00:00Z", "registered"))
            db.execute("DELETE FROM asset_trash WHERE asset_id=?", (identifier,))
            db.commit()
        backup.restore_assets(destination, destination / "archive", self.store, self.keys, [identifier])
        with closing(sqlite3.connect(target_db)) as db:
            self.assertEqual(db.execute("SELECT asset_id FROM asset_trash").fetchall(), [])
        restored_object = destination / "archive/objects/sha256" / digest[:2] / digest
        self.assertEqual(restored_object.read_bytes(), content)
        saved_slot = self.store.read(0)
        conflicting = backup._unpack(backup._decrypt(saved_slot, self.keys))
        conflicting[".momso-purged-originals.json"] = json.dumps([digest]).encode()
        self.store.write(0, backup._encrypt(backup._pack(conflicting), self.keys))
        with self.assertRaisesRegex(ValueError, "asset restore conflict"):
            backup.restore_assets(destination, destination / "archive", self.store, self.keys, [identifier])
        self.store.write(0, saved_slot)
        with closing(sqlite3.connect(target_db)) as db:
            db.execute("INSERT INTO asset_purged(asset_id) VALUES(?)", (identifier,))
            db.commit()
        restored_object.unlink()
        with self.assertRaisesRegex(ValueError, "asset permanently deleted"):
            backup.restore_assets(destination, destination / "archive", self.store, self.keys, [identifier])
        self.assertFalse(restored_object.exists())
        with closing(sqlite3.connect(target_db)) as db:
            db.execute("DELETE FROM asset_purged WHERE asset_id=?", (identifier,))
            db.commit()
        backup.restore_assets(destination, destination / "archive", self.store, self.keys, [identifier])
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

    def test_live_snapshot_remains_stable_after_user_continues(self):
        self.write_db("before")
        (self.data / "github-repository.json").write_text("before", encoding="utf8")
        digest = hashlib.sha256(b"original").hexdigest()
        source = self.data / "archive" / "objects" / "sha256" / digest[:2] / digest
        source.parent.mkdir(parents=True)
        source.write_bytes(b"original")
        staged = self.root / "snapshot"
        backup.stage_live_snapshot(self.data, staged)
        self.write_db("after")
        replacement = self.data / "replacement"
        replacement.write_text("after", encoding="utf8")
        replacement.replace(self.data / "github-repository.json")
        source.unlink()
        with closing(sqlite3.connect(staged / "server.sqlite3")) as db:
            self.assertEqual(db.execute("SELECT body FROM messages").fetchall(), [("before",)])
        self.assertEqual((staged / "github-repository.json").read_text(encoding="utf8"), "before")
        self.assertEqual((staged / "archive/objects/sha256" / digest[:2] / digest).read_bytes(), b"original")

    def test_live_snapshot_copies_objects_across_filesystems(self):
        digest = hashlib.sha256(b"example").hexdigest()
        source = self.data / "archive" / "objects" / "sha256" / digest[:2] / digest
        source.parent.mkdir(parents=True)
        source.write_bytes(b"example")
        staged = self.root / "snapshot"
        with mock.patch.object(backup.os, "link", side_effect=OSError(errno.EXDEV, "different filesystem")):
            backup.stage_live_snapshot(self.data, staged)
        self.assertEqual((staged / "archive/objects/sha256" / digest[:2] / digest).read_bytes(), b"example")

    @unittest.skipUnless(hasattr(os, "memfd_create"), "Linux in-memory file required")
    def test_live_snapshot_keeps_codex_auth_out_of_temporary_files(self):
        secret = os.urandom(32).hex().encode()
        (self.data / "codex").mkdir()
        (self.data / "codex/auth.json").write_bytes(secret)
        staged = self.root / "snapshot"
        memory = backup.stage_live_snapshot(self.data, staged)
        self.assertEqual(memory["codex/auth.json"], secret)
        self.assertFalse((staged / "codex/auth.json").exists())
        for path in staged.rglob("*"):
            if path.is_file():
                self.assertNotIn(secret, path.read_bytes())
        self.assertEqual(backup.backup(staged, self.store, self.keys, backup.DATABASE_AND_VAULT,
                                      archive=staged / "archive", quiescent=True,
                                      memory_files=memory), "uploaded")
        for _, value in self.store.objects.values():
            self.assertNotIn(secret, value)
        for value in self.store.chunks.values():
            self.assertNotIn(secret, value)
        restored = self.root / "restored"
        backup.restore(restored, self.store, self.keys, archive=restored / "archive")
        self.assertEqual((restored / "codex/auth.json").read_bytes(), secret)

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

    def test_backup_slot_replacement_requires_same_storage_generation(self):
        class Client:
            def __init__(self): self.calls = []
            def request(self, url, **kwargs):
                self.calls.append((url, kwargs))
                if kwargs.get("allow_missing"): return b'{"generation":"123"}'
                return b"encrypted fixture"
        client = Client()
        store = CloudStorage(client, "example-private-bucket")
        self.assertEqual(store.read_versioned(0), ("123", b"encrypted fixture"))
        self.assertIn("ifGenerationMatch=123", client.calls[-1][0])
        store.replace_if_generation(0, "123", b"replacement")
        self.assertIn("ifGenerationMatch=123", client.calls[-1][0])
        with self.assertRaises(ValueError): store.replace_if_generation(0, "1&other=1", b"replacement")
        self.assertTrue(store.delete_current_chunk_if_unchanged("a" * 32, 0))
        self.assertEqual(client.calls[-1][1]["method"], "DELETE")
        self.assertIn("ifGenerationMatch=123", client.calls[-1][0])
        store.delete_chunk_generation("a" * 32, 0, "123")
        self.assertEqual(client.calls[-1][1]["method"], "DELETE")
        self.assertIn("generation=123", client.calls[-1][0])
        self.assertIn("ifGenerationMatch=123", client.calls[-1][0])
        with self.assertRaises(ValueError): store.delete_chunk_generation("a" * 32, 0, "123&other=1")
        with self.assertRaises(ValueError): store.delete_current_chunk_if_unchanged("../escape", 0)

    def test_cloud_fence_recovery_never_releases_another_owner(self):
        class Client:
            def __init__(self): self.owner, self.generation, self.clock = None, None, 0
            def request(self, url, **kwargs):
                query = urllib.parse.parse_qs(urllib.parse.urlsplit(url).query)
                if "uploadType" in query:
                    self.assert_precondition(query, "0")
                    if self.owner is not None: raise GoogleBackupError("cloud_request_failed_412")
                    self.clock += 1
                    self.generation = str(self.clock)
                    self.owner = kwargs["data"]
                    return json.dumps({"generation": self.generation}).encode()
                if self.owner is None: raise GoogleBackupError("cloud_request_failed_404")
                if "ifGenerationMatch" in query and query["ifGenerationMatch"] != [self.generation]:
                    raise GoogleBackupError("cloud_request_failed_412")
                if kwargs.get("method") == "DELETE":
                    self.owner, self.generation = None, None
                    return b""
                if query.get("alt") == ["media"]: return self.owner
                return json.dumps({"generation": self.generation}).encode()
            @staticmethod
            def assert_precondition(query, expected):
                if query.get("ifGenerationMatch") != [expected]: raise AssertionError("missing condition")
        client = Client()
        store = CloudStorage(client, "example-private-bucket")
        first, other = "a" * 32, "b" * 32
        generation = store.acquire_fence(first)
        self.assertEqual(store.acquire_fence(first), generation)
        with self.assertRaisesRegex(GoogleBackupError, "backup_fence_busy"):
            store.acquire_fence(other)
        with self.assertRaisesRegex(GoogleBackupError, "backup_fence_busy"):
            store.release_fence(other, generation)
        store.release_fence(first, generation)
        next_generation = store.acquire_fence(other)
        with self.assertRaisesRegex(GoogleBackupError, "cloud_request_failed_412"):
            store.release_fence(first, generation)
        self.assertEqual(store.acquire_fence(other), next_generation)

    def test_cloud_purge_ledger_uses_generation_condition(self):
        class Client:
            def __init__(self): self.calls = []
            def request(self, url, **kwargs):
                self.calls.append((url, kwargs))
                if kwargs.get("allow_missing") and "alt=media" not in url:
                    return b'{"generation":"17"}'
                if "alt=media" in url:
                    return b"encrypted ledger"
                return b'{}'
        client = Client()
        store = CloudStorage(client, "example-private-bucket")
        self.assertEqual(store.read_purge_ledger_versioned(), ("17", b"encrypted ledger"))
        self.assertIn("ifGenerationMatch=17", client.calls[-1][0])
        store.write_purge_ledger_if_generation("17", b"next encrypted ledger")
        self.assertIn("ifGenerationMatch=17", client.calls[-1][0])
        self.assertNotIn("next encrypted ledger", client.calls[-1][0])
        with self.assertRaises(ValueError):
            store.write_purge_ledger_if_generation("17&unsafe", b"fixture")

    def test_chunk_listing_checks_all_pages_and_exact_name(self):
        class Client:
            def __init__(self): self.calls = []
            def request(self, url, **kwargs):
                self.calls.append(url)
                name = "momso-backup/chunks/" + "a" * 32 + "/0"
                if "pageToken=" not in url:
                    return json.dumps({"items":[{"name":name,"generation":"1"},{"name":name+"0","generation":"2"}],"nextPageToken":"next"}).encode()
                return json.dumps({"items":[{"name":name,"generation":"3","hardDeleteTime":"2026-11-30T00:00:00Z"}]}).encode()
        client = Client()
        store = CloudStorage(client, "example-private-bucket")
        found = store.list_chunk_generations("a" * 32, 0, soft_deleted=True)
        self.assertEqual([item["generation"] for item in found], ["1", "3"])
        self.assertEqual(found[1]["hardDeleteTime"], "2026-11-30T00:00:00Z")
        self.assertTrue(all("softDeleted=true" in call for call in client.calls))
        self.assertTrue(all("versions=true" not in call for call in client.calls))

    def test_bucket_retention_does_not_guess_missing_policy(self):
        class Client:
            def __init__(self, payload): self.payload = payload
            def request(self, url, **kwargs): return json.dumps(self.payload).encode()
        empty = CloudStorage(Client({}), "example-private-bucket").bucket_retention()
        self.assertIsNone(empty["softDeleteSeconds"])
        self.assertIsNone(empty["versioningEnabled"])
        policy = CloudStorage(Client({"softDeletePolicy":{"retentionDurationSeconds":"604800"}, "versioning":{"enabled":True}}), "example-private-bucket").bucket_retention()
        self.assertEqual(policy, {"softDeleteSeconds":604800, "versioningEnabled":True})
        with self.assertRaisesRegex(Exception, "invalid_storage_metadata"):
            CloudStorage(Client({"softDeletePolicy":{"retentionDurationSeconds":"not-a-number"}}), "example-private-bucket").bucket_retention()

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
        creates = [call for call in provider.calls if call[:3] == ("storage", "buckets", "create")]
        updates = [call for call in provider.calls if call[:3] == ("storage", "buckets", "update")]
        self.assertEqual(len(creates), 1)
        self.assertFalse(any(arg.startswith("--labels") or arg.startswith("--update-labels") for arg in creates[0]))
        self.assertEqual(updates, [("storage", "buckets", "update", "gs://momso-backup-abcdef123456", "--update-labels=momso-install-id=abcdef123456")])
        self.assertLess(provider.calls.index(creates[0]), provider.calls.index(updates[0]))
        self.assertEqual(set(result["bucket_permissions"]), {
            "storage.objects.get", "storage.objects.create", "storage.objects.delete"
        })
        bindings = [call for call in provider.calls if "add-iam-policy-binding" in call]
        self.assertEqual(len(bindings), 2)
        self.assertTrue(all("--member=serviceAccount:" + result["service_account"] in call for call in bindings))
        self.assertTrue(any("--public-access-prevention" in call for call in provider.calls))
        self.assertFalse(any("delete" == part for call in provider.calls for part in call))

    def test_failed_bucket_label_update_stops_before_granting_permissions(self):
        class Provider:
            install_id = "abcdef123456"
            project = "example-project"
            marker = "momso-install-" + install_id

            def __init__(self):
                self.calls = []

            def run(self, *args, **kwargs):
                self.calls.append(args)
                if args[:3] == ("storage", "buckets", "list"):
                    return []
                if args[:3] == ("storage", "buckets", "create"):
                    return b""
                if args[:3] == ("storage", "buckets", "update"):
                    raise InstallError("fixture label update failure")
                raise AssertionError("no permissions may be granted after label failure")

        provider = Provider()
        with self.assertRaises(InstallError):
            GoogleBackupProvisioner(provider).provision()
        self.assertEqual([call[:3] for call in provider.calls], [
            ("storage", "buckets", "list"),
            ("storage", "buckets", "create"),
            ("storage", "buckets", "update"),
        ])

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
