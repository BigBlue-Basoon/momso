import ast
import base64
import io
import json
import tempfile
import unittest
import urllib.error
import urllib.request
from pathlib import Path
from types import SimpleNamespace
from unittest.mock import patch

import momso_anywhere_install as anywhere
from momso_anywhere_public_assets import ASSETS

IMAGE = 'registry.example.test/momso/gateway@sha256:' + 'a' * 64


def public_fixture():
    files = {name: b"\x89PNG\r\n\x1a\nfixture" for name in ASSETS if name.endswith(".png")}
    files["manifest.webmanifest"] = json.dumps({"name": "momso", "start_url": "/", "scope": "/", "icons": [{"src": "/app-icons/icon-192.png"}]}).encode()
    return files


class Provider:
    project = "fixture-project"
    install_id = "0123456789ab"
    marker = "momso-install-0123456789ab"
    name = "momso-0123456789ab"
    network = name + "-net"
    subnet = name + "-sub"

    def __init__(self):
        self.calls = []
        self.account = self.role = self.service = None
        self.secrets = {}
        self.rule = None
        self.interrupt = False
        self.ssh_calls = []

    def ssh(self, code, data=None):
        self.ssh_calls.append((code, data))
        return json.dumps({"fingerprint": "a" * 64, "token": "b" * 64}).encode()

    def ensure(self, kind, name, list_args, create_args, vm=False):
        self.rule = {"name": name, "description": self.marker}

    def run(self, *args, data=None, parse=False):
        self.calls.append(args)
        if args[:3] == ("iam", "service-accounts", "list"): return [self.account] if self.account else []
        if args[:3] == ("iam", "service-accounts", "describe"): return self.account
        if args[:3] == ("iam", "roles", "list"): return [self.role] if self.role else []
        if args[:3] == ("iam", "roles", "describe"): return self.role
        if args[:3] == ("run", "services", "list"): return [self.service] if self.service else []
        if args[:3] == ("iam", "service-accounts", "create"):
            self.account = {"email": "ma-0123456789ab" + "@" + "fixture-project.iam.gserviceaccount.com", "description": self.marker}
        elif args[:3] == ("iam", "roles", "create"):
            self.role = {"name": "projects/fixture-project/roles/momsoAnywhere0123456789ab", "description": self.marker, "includedPermissions": ["compute.instances.get", "compute.instances.start"]}
        elif args[:2] == ("secrets", "list"):
            return [{"name": "projects/fixture/secrets/" + name, "labels": {"momso-install-id": self.install_id}} for name in self.secrets]
        elif args[:2] == ("secrets", "create"): self.secrets[args[2]] = None
        elif args[:3] == ("secrets", "versions", "list"): return [{"name": "1"}] if self.secrets[args[3]] else []
        elif args[:3] == ("secrets", "versions", "add"): self.secrets[args[3]] = data
        elif args[:3] == ("secrets", "versions", "access"): return self.secrets[args[4].split("=", 1)[1]]
        elif args[:2] == ("secrets", "delete"): self.secrets.pop(args[2])
        elif args[:3] == ("compute", "instances", "describe"): return {"networkInterfaces": [{"networkIP": "10.42.0.10"}]}
        elif args[:3] == ("compute", "firewall-rules", "list"): return [self.rule] if self.rule else []
        elif args[:3] == ("compute", "firewall-rules", "describe"): return {"description": self.marker, "direction": "INGRESS", "network": "projects/fixture/global/networks/" + self.network, "sourceRanges": ["10.42.0.0/24"], "targetTags": [self.name], "allowed": [{"IPProtocol": "tcp", "ports": ["8766"]}]}
        elif args[:3] == ("compute", "firewall-rules", "delete"): self.rule = None
        elif args[:2] == ("run", "deploy"):
            if self.interrupt:
                self.interrupt = False
                raise anywhere.InstallError("fixture interruption")
            self.service = {"metadata": {"name": "momso-any-0123456789ab", "labels": {"momso-install-id": self.install_id}}}
        elif args[:3] == ("run", "services", "describe"):
            value = {"status": {"url": "https://fixture.a.run.app", "traffic": [{"revisionName": "fixture-rev", "percent": 100}], "latestReadyRevisionName": "fixture-rev", "conditions": [{"type": "Ready", "status": "True"}]}}
            return value if parse else json.dumps(value).encode()
        elif args[:3] == ("run", "services", "delete"): self.service = None
        elif args[:3] == ("iam", "service-accounts", "delete"): self.account = None
        elif args[:3] == ("iam", "roles", "delete"): self.role = None
        return b""


class Tests(unittest.TestCase):
    def test_codex_install_also_installs_missing_git(self):
        from types import SimpleNamespace
        import inspect

        function = ast.parse(inspect.getsource(anywhere.install_codex)).body[0]
        code = next(node.value.value for node in function.body if isinstance(node, ast.Assign) and any(isinstance(target, ast.Name) and target.id == "code" for target in node.targets))
        tree = ast.parse(code)
        assignment = next(node for node in tree.body if isinstance(node, ast.Assign) and any(isinstance(target, ast.Name) and target.id == "missing" for target in node.targets))
        branch = next(node for node in tree.body if isinstance(node, ast.If) and isinstance(node.test, ast.Name) and node.test.id == "missing")
        for available, expected in [({"bwrap"}, ["git"]), ({"git"}, ["bubblewrap"]), (set(), ["bubblewrap", "git"]), ({"bwrap", "git"}, [])]:
            calls = []
            env = {"shutil": SimpleNamespace(which=lambda name: name if name in available else None), "os": SimpleNamespace(environ={}), "run": lambda *args: calls.append(args)}
            exec(compile(ast.Module(body=[assignment, branch], type_ignores=[]), "fixture", "exec"), env)
            self.assertEqual(env["missing"], expected)
            self.assertEqual(list(calls[-1][4:]) if calls else [], expected)

    def test_new_role_iam_propagation_is_retried(self):
        p = Provider()
        original = p.run
        attempts = []
        def flaky(*args, **kwargs):
            if args[:3] == ("compute", "instances", "add-iam-policy-binding"):
                attempts.append(1)
                if len(attempts) < 3:
                    raise anywhere.InstallError("fixture role not propagated", role_propagating=True)
            return original(*args, **kwargs)
        p.run = flaky
        with patch("momso_iam_retry.time.sleep") as sleep:
            anywhere.GoogleAnywhere(p).prepare_identity()
        self.assertEqual(len(attempts), 3)
        self.assertEqual(sleep.call_count, 2)

    def test_upgrade_redeploys_same_service_without_changing_settings(self):
        p = Provider()
        gateway = anywhere.GoogleAnywhere(p)
        gateway.upgrade_gateway("https://fixture.a.run.app", IMAGE)
        calls = [call for call in p.calls if call[:2] == ("run", "deploy")]
        self.assertEqual(len(calls), 1)
        self.assertEqual(calls[0][2], gateway.service)
        self.assertIn(IMAGE, calls[0])
        self.assertFalse(any(flag.startswith('--source') for flag in calls[0]))
        self.assertFalse(any(flag.startswith(("--set-env-vars", "--set-secrets", "--allow-unauthenticated")) for flag in calls[0]))

    def test_upgrade_failure_restores_the_previous_revision_traffic(self):
        provider = Provider()
        gateway = anywhere.GoogleAnywhere(provider)
        previous = {'traffic': {'fixture-old': 100}}
        adapter = SimpleNamespace(current=lambda: previous, healthy=lambda: False)
        calls = []
        def activate(release, **kwargs): calls.append(release)
        adapter.activate = activate
        with patch.object(anywhere, 'cloud_run_gateway', return_value=adapter):
            with self.assertRaises(anywhere.InstallError):
                gateway.upgrade_gateway('https://fixture.a.run.app', IMAGE)
        self.assertEqual(calls, [IMAGE, previous])

    def test_new_role_iam_retry_is_bounded(self):
        p = Provider()
        original = p.run
        attempts = []
        def fail(*args, **kwargs):
            if args[:3] == ("compute", "instances", "add-iam-policy-binding"):
                attempts.append(1)
                raise anywhere.InstallError("fixture access denied")
            return original(*args, **kwargs)
        p.run = fail
        with patch("momso_iam_retry.time.sleep"):
            with self.assertRaises(anywhere.InstallError):
                anywhere.GoogleAnywhere(p).prepare_identity()
        self.assertEqual(len(attempts), 1)

    def test_codex_installer_pins_official_archive_without_reading_user_login(self):
        p = Provider()
        anywhere.install_codex(p)
        wrapper = p.ssh_calls[0][0]
        tree = ast.parse(wrapper)
        encoded = next(node.args[0].value for node in ast.walk(tree) if isinstance(node, ast.Call) and isinstance(node.func, ast.Attribute) and node.func.attr == "b64decode")
        payload = base64.b64decode(encoded).decode()
        ast.parse(payload)
        self.assertIn("rust-v0.169.0", payload)
        self.assertIn("9af4d751fce183faedea629437d5252dbf458eeb04130cf4f97be844621ea985", payload)
        self.assertIn("MOMSO_CODEX_PATH=/opt/momso/codex", payload)

    def test_private_vm_bootstrap_is_valid_and_never_puts_token_in_command(self):
        p = Provider()
        result = anywhere.install_private_proxy(p)
        wrapper, data = p.ssh_calls[0]
        self.assertNotIn(result["token"], wrapper)
        self.assertIn("momso_anywhere_vm_proxy.py", json.loads(data))
        tree = ast.parse(wrapper)
        encoded = next(node.args[0].value for node in ast.walk(tree) if isinstance(node, ast.Call) and isinstance(node.func, ast.Attribute) and node.func.attr == "b64decode")
        payload = base64.b64decode(encoded).decode()
        parsed = ast.parse(payload)
        unit_expr = next(node.value for node in parsed.body if isinstance(node, ast.Assign) and isinstance(node.targets[0], ast.Name) and node.targets[0].id == "unit")
        for base, expected in (("/opt/momso", "/opt/momso"), ("/opt/momso/releases/version-2", "/opt/momso/releases/current")):
            unit = eval(compile(ast.Expression(unit_expr), "fixture", "eval"), {"__builtins__": {}}, {"base": base})
            self.assertIn("\nExecStart=/usr/bin/python3 " + expected + "/momso_anywhere_vm_proxy.py\n", unit)
            self.assertEqual("Requires=momso-server.service" in unit, base == "/opt/momso")

    def test_private_proxy_health_waits_for_connection(self):
        p = Provider()
        anywhere.install_private_proxy(p)
        wrapper = ast.parse(p.ssh_calls[0][0])
        encoded = next(node.args[0].value for node in ast.walk(wrapper) if isinstance(node, ast.Call) and isinstance(node.func, ast.Attribute) and node.func.attr == "b64decode")
        parsed = ast.parse(base64.b64decode(encoded).decode())
        loop = next(node for node in parsed.body if isinstance(node, ast.While))
        calls = []

        def urlopen(*args, **kwargs):
            calls.append(1)
            if len(calls) < 3: raise OSError("fixture connection refused")
            return io.BytesIO(b'{"state":"ready"}')

        with tempfile.TemporaryDirectory() as directory:
            token = Path(directory) / "token"
            token.write_text("fixture", encoding="ascii")
            network = SimpleNamespace(request=SimpleNamespace(Request=urllib.request.Request, urlopen=urlopen), error=urllib.error)
            clock = SimpleNamespace(monotonic=lambda: 1, sleep=lambda _: None)
            env = {"urllib": network, "time": clock, "deadline": 30, "token": str(token), "ctx": None, "json": json}
            exec(compile(ast.Module(body=[loop], type_ignores=[]), "fixture", "exec"), env)
        self.assertEqual(len(calls), 3)

    def test_customer_resources_resume_and_destroy_without_revealing_secrets(self):
        p = Provider()
        proxy = {"fingerprint": "a" * 64, "token": "b" * 64}
        install = anywhere.GoogleAnywhere(p)
        p.interrupt = True
        with self.assertRaises(anywhere.InstallError): install.install(proxy, "fixture.apps.googleusercontent.com", "12345", "https://login.example.test", IMAGE)
        self.assertEqual(install.install(proxy, "fixture.apps.googleusercontent.com", "12345", "https://login.example.test", IMAGE), "https://fixture.a.run.app")
        self.assertFalse(any(proxy["token"] in " ".join(call) for call in p.calls))
        deploys = [call for call in p.calls if call[:2] == ("run", "deploy")]
        self.assertIn("--no-allow-unauthenticated", deploys[-2])
        self.assertIn("--allow-unauthenticated", deploys[-1])
        self.assertIn("--timeout=3600", deploys[-1])
        install.destroy()
        self.assertFalse(p.secrets)
        self.assertIsNone(p.rule)

    def test_existing_unowned_secret_is_preserved(self):
        p = Provider()
        install = anywhere.GoogleAnywhere(p)
        p.run = lambda *args, **kwargs: [{"name": "projects/fixture/secrets/" + install.transport_secret, "labels": {}}] if args[:2] == ("secrets", "list") else []
        with self.assertRaises(anywhere.InstallError): install.ensure_secret(install.transport_secret, b"fixture")

    def test_gateway_accepts_cloud_run_static_login_origin(self):
        p = Provider()
        proxy = {"fingerprint": "a" * 64, "token": "b" * 64}
        origin = "https://static-123.asia-northeast3.run.app"
        gateway = anywhere.GoogleAnywhere(p)
        self.assertEqual(gateway.install(proxy, "fixture.apps.googleusercontent.com", "12345", origin, IMAGE), "https://fixture.a.run.app")
        deploys = [call for call in p.calls if call[:2] == ("run", "deploy")]
        self.assertTrue(any("MOMSO_LOGIN_ORIGIN=" + origin in part for part in deploys[-1]))


if __name__ == "__main__": unittest.main()
