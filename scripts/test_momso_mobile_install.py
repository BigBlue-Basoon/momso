import ast
import base64
import hashlib
import importlib.util
import io
import json
import tempfile
import unittest
import urllib.error
from pathlib import Path
from unittest.mock import patch

import momso_mobile_install as mobile

ACCOUNT = "a" * 32
ZONE = "b" * 32
TUNNEL = "1" * 8 + "-" + "2" * 4 + "-" + "3" * 4 + "-" + "4" * 4 + "-" + "5" * 12
APP = "6" * 8 + "-" + "7" * 4 + "-" + "8" * 4 + "-" + "9" * 4 + "-" + "a" * 12
POLICY = "b" * 8 + "-" + "c" * 4 + "-" + "d" * 4 + "-" + "e" * 4 + "-" + "f" * 12
TOKEN = "fixture-tunnel-token-not-real"
EMAIL = "fixture@example.test"
HOST = "mobile.example.test"


class FakeCloudflare:
    def __init__(self):
        self.app = self.tunnel = self.record = self.config = None
        self.policy = [{"id": POLICY, "decision": "allow", "include": [{"email": {"email": EMAIL}}]}]
        self.tamper_config = False
        self.policy_pages = 1
        self.operations = []

    def __call__(self, method, path, body):
        self.operations.append((method, path))
        clean = path.split("?")[0]
        if clean == "/zones/" + ZONE:
            value = {"status": "active", "account": {"id": ACCOUNT}, "name": "example.test"}
        elif clean.endswith("/access/organizations"):
            value = {"auth_domain": "fixture.cloudflareaccess.com"}
        elif clean.endswith("/access/apps"):
            if method == "POST":
                self.app = {"id": APP, "name": body["name"], "domain": body["domain"], "aud": "c" * 64}
                value = self.app
            else:
                value = [self.app] if self.app else []
        elif clean.endswith("/access/apps/" + APP + "/policies"):
            value = self.policy
        elif clean.endswith("/access/apps/" + APP + "/policies/" + POLICY) and method == "DELETE":
            self.policy = []
            value = {"id": POLICY}
        elif clean.endswith("/access/apps/" + APP) and method == "DELETE":
            self.app = None
            value = {"id": APP}
        elif clean.endswith("/cfd_tunnel"):
            if method == "POST":
                self.tunnel = {"id": TUNNEL, "name": body["name"], "config_src": "cloudflare"}
                value = self.tunnel
            else:
                value = [self.tunnel] if self.tunnel else []
        elif clean.endswith("/cfd_tunnel/" + TUNNEL + "/configurations"):
            if method == "PUT": self.config = body["config"]
            value = {"config": self.config}
            if self.tamper_config and method == "GET":
                value = json.loads(json.dumps(value))
                value["config"]["ingress"][0]["originRequest"]["access"]["required"] = False
        elif clean.endswith("/dns_records"):
            if method == "POST":
                self.record = dict(body, id="d" * 32)
                value = self.record
            else:
                value = [self.record] if self.record else []
        elif clean.endswith("/cfd_tunnel/" + TUNNEL + "/token"):
            value = TOKEN
        elif clean.endswith("/cfd_tunnel/" + TUNNEL + "/connections") and method == "DELETE":
            value = {}
        elif clean.endswith("/cfd_tunnel/" + TUNNEL) and method == "DELETE":
            self.tunnel = None
            value = {"id": TUNNEL}
        elif clean.endswith("/dns_records/" + "d" * 32) and method == "DELETE":
            self.record = None
            value = {}
        else:
            raise AssertionError((method, path))
        pages = self.policy_pages if clean.endswith("/access/apps/" + APP + "/policies") else 1
        return {"success": True, "result": value, "result_info": {"total_pages": pages}}


class FakeVM:
    def __init__(self): self.calls = []
    def ssh(self, code, data=None): self.calls.append((code, data))


class Tests(unittest.TestCase):
    def setUp(self):
        self.tmp = tempfile.TemporaryDirectory()
        self.addCleanup(self.tmp.cleanup)
        self.path = Path(self.tmp.name) / "state.json"
        self.state = {"id": "0123456789ab", "steps": {}}
        self.remote = FakeCloudflare()
        self.cf = mobile.Cloudflare("fixture-api-token", ACCOUNT, ZONE, self.remote)

    def test_policy_and_tunnel_configuration_reconcile_without_secrets(self):
        for _ in range(2):
            token, team = mobile.configure(self.cf, self.path, self.state, HOST, EMAIL)
            self.assertEqual((token, team), (TOKEN, "fixture.cloudflareaccess.com"))
        self.assertEqual(sum(method == "POST" and path.endswith("/access/apps") for method, path in self.remote.operations), 1)
        self.assertEqual(sum(method == "POST" and path.endswith("/cfd_tunnel") for method, path in self.remote.operations), 1)
        self.assertNotIn(TOKEN, self.path.read_text(encoding="utf-8"))
        self.assertTrue(self.remote.config["ingress"][0]["originRequest"]["access"]["required"])

    def test_rejects_existing_dns_before_changing_cloud_resources(self):
        self.remote.record = {"name": HOST, "comment": "other"}
        with self.assertRaises(mobile.InstallError):
            mobile.configure(self.cf, self.path, self.state, HOST, EMAIL)
        self.assertFalse(any(method in ("POST", "PUT") for method, _ in self.remote.operations))

    def test_same_named_unowned_app_is_not_adopted(self):
        self.remote.app = {"id": APP, "name": "momso-0123456789ab", "domain": HOST, "aud": "c" * 64}
        with self.assertRaises(mobile.InstallError):
            mobile.configure(self.cf, self.path, self.state, HOST, EMAIL)
        self.assertFalse(any(method in ("POST", "PUT") for method, _ in self.remote.operations))

    def test_rejects_broad_policy_and_unverified_tunnel(self):
        self.remote.policy = [{"decision": "allow", "include": [{"everyone": {}}]}]
        with self.assertRaises(mobile.InstallError):
            mobile.configure(self.cf, self.path, self.state, HOST, EMAIL)
        self.assertFalse(any(path.endswith("/cfd_tunnel") and method == "POST" for method, path in self.remote.operations))
        self.remote.policy = [{"id": POLICY, "decision": "allow", "include": [{"email": {"email": EMAIL}}]}]
        self.remote.policy_pages = 2
        with self.assertRaises(mobile.InstallError):
            mobile.configure(self.cf, self.path, self.state, HOST, EMAIL)
        self.remote.policy_pages = 1
        self.remote.tamper_config = True
        with self.assertRaises(mobile.InstallError):
            mobile.configure(self.cf, self.path, self.state, HOST, EMAIL)
        self.assertIsNone(self.remote.record)

    def test_connect_never_puts_token_in_command_or_state(self):
        vm = FakeVM()
        binary = b"fictional cloudflared"
        error = urllib.error.HTTPError("https://fixture.invalid", 302, "redirect", {"Location": "https://fixture.cloudflareaccess.com/login"}, io.BytesIO())
        with patch.object(mobile, "RELEASE_SHA256", hashlib.sha256(binary).hexdigest()), patch.object(mobile.urllib.request, "build_opener") as opener:
            opener.return_value.open.side_effect = error
            mobile.connect(vm, self.cf, self.path, self.state, HOST, EMAIL, connector=binary)
        self.assertEqual(len(vm.calls), 2)
        self.assertNotIn(TOKEN, vm.calls[0][0])
        self.assertIn(TOKEN.encode(), vm.calls[0][1])
        self.assertNotIn(TOKEN, self.path.read_text(encoding="utf-8"))
        wrapper = ast.parse(vm.calls[0][0])
        encoded = next(item.value.value for item in wrapper.body if isinstance(item, ast.Assign) and isinstance(item.targets[0], ast.Name) and item.targets[0].id == "p")
        compile(base64.b64decode(encoded), "vm-bootstrap", "exec")
        ready = ast.parse(vm.calls[1][0])
        code = next(item.value.value for item in ready.body if isinstance(item, ast.Assign) and isinstance(item.targets[0], ast.Name) and item.targets[0].id == "code")
        parsed = ast.parse(code)
        constants = [node.value for node in ast.walk(parsed) if isinstance(node, ast.Constant) and isinstance(node.value, bytes)]
        self.assertIn(b'GET /health HTTP/1.0\r\nHost: mobile.example.test\r\n\r\n', constants)
        self.assertIn(b'"state"', constants)

    def test_unprotected_public_response_stops_tunnel(self):
        vm = FakeVM()
        binary = b"fictional cloudflared"
        with patch.object(mobile, "RELEASE_SHA256", hashlib.sha256(binary).hexdigest()), patch.object(mobile.urllib.request, "build_opener"):
            with self.assertRaises(mobile.InstallError):
                mobile.connect(vm, self.cf, self.path, self.state, HOST, EMAIL, connector=binary)
        self.assertIn("stop", vm.calls[-1][0])
        self.assertNotIn("configured", self.state["mobile"])

    def test_retry_after_tunnel_creation_response_is_lost(self):
        original = self.remote
        lost = {"once": True}
        def interrupted(method, path, body):
            result = original(method, path, body)
            if lost["once"] and method == "POST" and path.endswith("/cfd_tunnel"):
                lost["once"] = False
                raise mobile.InstallError("fixture connection interrupted")
            return result
        self.cf.transport = interrupted
        with self.assertRaises(mobile.InstallError):
            mobile.configure(self.cf, self.path, self.state, HOST, EMAIL)
        mobile.configure(self.cf, self.path, self.state, HOST, EMAIL)
        self.assertEqual(sum(method == "POST" and path.endswith("/cfd_tunnel") for method, path in original.operations), 1)

    def test_public_unexpected_error_stops_tunnel(self):
        vm = FakeVM()
        binary = b"fictional cloudflared"
        error = urllib.error.HTTPError("https://fixture.invalid", 502, "gateway", {}, io.BytesIO())
        with patch.object(mobile, "RELEASE_SHA256", hashlib.sha256(binary).hexdigest()), patch.object(mobile.urllib.request, "build_opener") as opener:
            opener.return_value.open.side_effect = error
            with self.assertRaises(mobile.InstallError):
                mobile.connect(vm, self.cf, self.path, self.state, HOST, EMAIL, connector=binary)
        self.assertIn("stop", vm.calls[-1][0])

    def test_destroy_owned_is_idempotent_and_removes_access_last(self):
        mobile.configure(self.cf, self.path, self.state, HOST, EMAIL)
        calls = []
        for _ in range(2):
            mobile.destroy_owned(self.cf, self.path, self.state, HOST, EMAIL, lambda: calls.append("vm"))
        self.assertEqual(calls, ["vm", "vm"])
        deleted = [path for method, path in self.remote.operations if method == "DELETE"]
        self.assertIn("/dns_records/", deleted[0])
        self.assertTrue(deleted[-1].endswith("/access/apps/" + APP))
        self.assertTrue(self.state["mobile"]["destroyed"])

    def test_destroy_refuses_modified_resources_without_deleting_anything(self):
        mobile.configure(self.cf, self.path, self.state, HOST, EMAIL)
        self.remote.record["comment"] = "someone-else"
        with self.assertRaises(mobile.InstallError):
            mobile.destroy_owned(self.cf, self.path, self.state, HOST, EMAIL, lambda: self.fail("vm deleted"))
        self.assertFalse(any(method == "DELETE" for method, _ in self.remote.operations))

    def test_destroy_refuses_a_policy_added_by_someone_else(self):
        mobile.configure(self.cf, self.path, self.state, HOST, EMAIL)
        self.remote.policy.append({"id": "0" * 36})
        with self.assertRaises(mobile.InstallError):
            mobile.destroy_owned(self.cf, self.path, self.state, HOST, EMAIL, lambda: self.fail("vm deleted"))
        self.assertFalse(any(method == "DELETE" for method, _ in self.remote.operations))

    def test_destroy_resumes_after_tunnel_delete_fails(self):
        mobile.configure(self.cf, self.path, self.state, HOST, EMAIL)
        original = self.remote
        failed = {"once": True}
        def interrupted(method, path, body):
            if failed["once"] and method == "DELETE" and path.endswith("/cfd_tunnel/" + TUNNEL):
                failed["once"] = False
                raise mobile.InstallError("fixture interruption")
            return original(method, path, body)
        self.cf.transport = interrupted
        with self.assertRaises(mobile.InstallError):
            mobile.destroy_owned(self.cf, self.path, self.state, HOST, EMAIL, lambda: None)
        self.assertIsNone(self.remote.record)
        self.assertIsNotNone(self.remote.app)
        mobile.destroy_owned(self.cf, self.path, self.state, HOST, EMAIL, lambda: None)
        self.assertIsNone(self.remote.tunnel)
        self.assertIsNone(self.remote.app)


if __name__ == "__main__":
    unittest.main()
