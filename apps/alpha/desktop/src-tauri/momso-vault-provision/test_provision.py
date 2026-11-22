from pathlib import Path
import os
import subprocess
import base64
import json
from unittest.mock import patch
from urllib.error import HTTPError
import tempfile
import unittest
from provision import SecretManager, VaultProvisionError, prepare


class FakeStore:
    def __init__(self): self.identity = None; self.writes = 0
    def get(self): return self.identity
    def add(self, value): self.identity = value; self.writes += 1


class ProvisionTests(unittest.TestCase):
    def test_first_install_restart_and_new_vm_use_same_identity(self):
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory); store = FakeStore(); secret = next(line for line in subprocess.run(["age-keygen"], capture_output=True, check=True).stdout.splitlines() if line.startswith(b"AGE-SECRET-KEY-1"))
            destination = root / "run" / "identity"
            prepare(store, destination, root / "vault", create=True, generator=lambda: b"# created: fixture\n# public key: fixture\n" + secret + b"\n")
            self.assertEqual(destination.read_bytes().strip(), secret); self.assertEqual(store.writes, 1)
            destination.with_name(destination.name + ".new").write_bytes(b"interrupted temporary file")
            prepare(store, destination, root / "vault", create=True, generator=lambda: self.fail("unexpected regeneration"))
            prepare(store, root / "other-vm" / "identity", root / "restored", generator=lambda: self.fail("unexpected regeneration"))
            self.assertEqual((root / "other-vm" / "identity").read_bytes().strip(), secret); self.assertEqual(store.writes, 1)
            self.assertEqual(destination.stat().st_mode & 0o777, 0o600)
    def test_secret_manager_api_first_create_and_restore(self):
        token = os.urandom(16).hex()
        state = {"identity": None, "adds": 0}
        def fake_request(url, headers, data=None):
            if "metadata.google.internal" in url:
                return json.dumps({"access_token": token}).encode()
            self.assertEqual(headers["Authorization"], "Bearer " + token)
            if url.endswith("/versions/1:access"):
                if state["identity"] is None:
                    raise HTTPError(url, 404, "fixture", {}, None)
                return json.dumps({"payload":{"data":base64.b64encode(state["identity"]).decode()}}).encode()
            self.assertTrue(url.endswith(":addVersion"))
            state["identity"] = base64.b64decode(json.loads(data)["payload"]["data"])
            state["adds"] += 1
            return b"{}"
        with tempfile.TemporaryDirectory() as directory, patch("provision.request", side_effect=fake_request):
            root = Path(directory); store = SecretManager("fixture-project", "fixture-secret")
            prepare(store, root / "run" / "identity", root / "vault", create=True)
            first = (root / "run" / "identity").read_bytes()
            prepare(store, root / "new-vm" / "identity", root / "restored")
            self.assertEqual(first, (root / "new-vm" / "identity").read_bytes())
            self.assertEqual(state["adds"], 1)

    def test_existing_encrypted_vault_never_generates_replacement_key(self):
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory); vault = root / "vault"; vault.mkdir(); (vault / "vault.age").write_bytes(b"ciphertext")
            store = FakeStore()
            with self.assertRaisesRegex(VaultProvisionError, "credential_missing"):
                prepare(store, root / "run" / "identity", vault, create=True, generator=lambda: self.fail("regenerated"))
            self.assertEqual(store.writes, 0)
    def test_missing_remote_key_without_initial_install_fails(self):
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            with self.assertRaisesRegex(VaultProvisionError, "credential_missing"):
                prepare(FakeStore(), root / "run" / "identity", root / "vault")


if __name__ == "__main__": unittest.main()
