import ast
import base64
import hashlib
import importlib.util
import io
import json
import os
import pathlib
import subprocess
import tarfile
import tempfile
import unittest
import momso_release_manifest
import momso_release_keys
from pathlib import Path
from types import SimpleNamespace
from unittest.mock import patch

spec = importlib.util.spec_from_file_location("installer", Path(__file__).with_name("momso_server_install.py"))
installer = importlib.util.module_from_spec(spec)
spec.loader.exec_module(installer)


class FakeProvider:
    def __init__(self, failure=None):
        self.failure = failure
        self.resources = set()
        self.calls = []

    def step(self, name):
        self.calls.append(name)
        if self.failure == name:
            self.failure = None
            raise installer.InstallError("fixture failure")
        self.resources.add(name)

    def prepare_network(self): self.step("network")
    def prepare_vm(self): self.step("vm")
    def install(self, artifact, digest):
        assert hashlib.sha256(artifact.read_bytes()).hexdigest() == digest
        self.step("install")
    def health(self): self.step("health")
    def install_web(self, archive, digest):
        assert hashlib.sha256(archive.read_bytes()).hexdigest() == digest
        self.step("web")
    def destroy(self): self.resources.clear()


class SimulatedCloud:
    def __init__(self, provider):
        self.provider = provider
        self.resources = {}
        self.nats = set()
        self.fail_vm_once = False
        self.creates = []

    def __call__(self, *args, data=None, parse=False):
        del data, parse
        if args[1:3] == ("routers", "nats"):
            if args[3] == "create": self.nats.add(args[4])
            elif args[3] == "delete": self.nats.discard(args[4])
            return b""
        if args[1] in ("ssh", "scp"):
            return b""
        if args[1:3] == ("networks", "subnets"):
            kind, op, rest = "subnets", args[3], args[4:]
        else:
            kind, op, rest = args[1], args[2], args[3:]
        if op == "list":
            return [value for (resource, _), value in self.resources.items() if resource == kind]
        if op == "describe":
            value = dict(self.resources[(kind, rest[0])])
            if kind == "routers": value["nats"] = [{"name": name} for name in self.nats]
            return value
        if op == "create":
            if kind == "instances" and self.fail_vm_once:
                self.fail_vm_once = False
                raise installer.InstallError("fixture interruption")
            name = rest[0]
            item = {"name": name, "description": self.provider.marker}
            if kind == "instances":
                item.update({"labels": {"momso-install-id": self.provider.install_id}, "networkInterfaces": [{"network": "/networks/" + self.provider.network}], "disks": [{"boot": True, "source": "/disks/" + name}], "status": "RUNNING"})
            self.resources[(kind, name)] = item
            self.creates.append((kind, name))
            return b""
        if op == "delete":
            self.resources.pop((kind, rest[0]))
            return b""
        raise AssertionError(args)


class FlowTests(unittest.TestCase):
    def setUp(self):
        self.tmp = tempfile.TemporaryDirectory()
        self.addCleanup(self.tmp.cleanup)
        self.root = Path(self.tmp.name)
        self.artifact = self.root / "momso-server"
        self.artifact.write_bytes(b"fictional server")
        self.digest = hashlib.sha256(self.artifact.read_bytes()).hexdigest()
        self.path = self.root / "state.json"
        self.state = installer.initialize_state(self.path, "fixture@example.test", "fixture-project")
        self.public_key = self.root / 'test-public.pem'
        self.public_key.write_text('fixture')
        keys = patch.object(momso_release_keys, 'validate', return_value=b'fixture')
        keys.start(); self.addCleanup(keys.stop)
        image = 'registry.example.test/momso/gateway@sha256:' + 'a' * 64
        for name, value in [('release', {'gatewayImage': image, 'bundleUrl': 'https://example.test/bundle', 'bundleSha256': 'b' * 64, 'sequence': 2}), ('download', b'fixture'), ('verify_inputs', None)]:
            mock = patch.object(momso_release_manifest, name, return_value=value)
            mock.start()
            self.addCleanup(mock.stop)
        import momso_anywhere_update_install
        self.update_grant = patch.object(momso_anywhere_update_install, "grant_gateway_update")
        self.update_install = patch.object(momso_anywhere_update_install, "install")
        self.update_grant_mock = self.update_grant.start(); self.addCleanup(self.update_grant.stop)
        self.update_install_mock = self.update_install.start(); self.addCleanup(self.update_install.stop)

    def test_success_retry_and_destroy(self):
        provider = FakeProvider()
        installer.deploy(provider, self.artifact, self.digest, self.path, self.state)
        self.assertEqual(provider.resources, {"network", "vm", "install", "health"})
        installer.deploy(provider, self.artifact, self.digest, self.path, self.state)
        self.assertEqual(provider.calls.count("health"), 2)
        provider.destroy()
        self.assertFalse(provider.resources)

    def test_interrupted_step_resumes_without_duplicate_resources(self):
        provider = FakeProvider("install")
        with self.assertRaises(installer.InstallError):
            installer.deploy(provider, self.artifact, self.digest, self.path, self.state)
        self.assertEqual(json.loads(self.path.read_text(encoding="utf-8"))["steps"]["install"], "failed")
        state = installer.initialize_state(self.path, "fixture@example.test", "fixture-project")
        installer.deploy(provider, self.artifact, self.digest, self.path, state)
        self.assertEqual(len(provider.resources), 4)

    def test_wrong_hash_stops_before_creating_resources(self):
        provider = FakeProvider()
        with self.assertRaises(installer.InstallError):
            installer.deploy(provider, self.artifact, "0" * 64, self.path, self.state)
        self.assertFalse(provider.calls)

    def test_existing_unowned_resource_is_not_changed(self):
        with patch.object(installer.shutil, "which", return_value="gcloud"):
            provider = installer.GoogleProvider("fixture@example.test", "fixture-project", "0123456789ab", self.root / "fixture-key")
        with patch.object(provider, "run", return_value=[{"name": provider.network, "description": "unrelated"}]) as run:
            with self.assertRaises(installer.InstallError):
                provider.prepare_network()
            self.assertEqual(run.call_count, 1)

    def test_every_gcloud_call_has_explicit_account_and_project(self):
        with patch.object(installer.shutil, "which", return_value="gcloud"):
            provider = installer.GoogleProvider("fixture@example.test", "fixture-project", "0123456789ab", self.root / "fixture-key")
        result = subprocess.CompletedProcess([], 0, stdout=b"[]", stderr=b"")
        with patch.object(installer.subprocess, "run", return_value=result) as run:
            provider.run("compute", "networks", "list", parse=True)
            args = run.call_args.args[0]
            self.assertIn("--account=fixture@example.test", args)
            self.assertIn("--project=fixture-project", args)

    def test_wrong_account_cannot_reuse_state(self):
        with self.assertRaises(installer.InstallError):
            installer.initialize_state(self.path, "other@example.test", "fixture-project")

    def test_private_vm_and_explicit_ssh_settings(self):
        with patch.object(installer.shutil, "which", return_value="gcloud"):
            provider = installer.GoogleProvider("fixture@example.test", "fixture-project", "0123456789ab", self.root / "fixture-key")
        with patch.object(provider, "ensure") as ensure:
            with patch.object(provider, "run", return_value={"labels":{"momso-install-id":"0123456789ab"},"networkInterfaces":[{"network":"projects/fixture/global/networks/" + provider.network}],"status":"RUNNING"}):
                provider.prepare_vm()
            flags = ensure.call_args.args[3]
            self.assertIn("--no-address", flags)
            self.assertIn("--no-service-account", flags)
            self.assertIn("--metadata=enable-oslogin=FALSE,block-project-ssh-keys=TRUE,enable-guest-attributes=TRUE", flags)
        with patch.object(provider, "run", return_value=b"") as run:
            provider.ssh("print('fixture')")
            flags = run.call_args_list[0].args
            self.assertIn("--tunnel-through-iap", flags)
            self.assertIn("--strict-host-key-checking=yes", flags)
            self.assertTrue(any(arg.startswith("--ssh-key-file=") for arg in flags))

    def test_ssh_binary_input_uses_iap_scp_and_verifies_hash(self):
        with patch.object(installer.shutil, "which", return_value="gcloud"):
            provider = installer.GoogleProvider("fixture@example.test", "fixture-project", "0123456789ab", self.root / "fixture-key")
        calls = []
        payload = bytes(range(256))

        def run(*args, **kwargs):
            calls.append(args)
            if args[:2] == ("compute", "scp"):
                package = Path(args[2]).read_bytes()
                length = int.from_bytes(package[:8], 'big')
                self.assertEqual(package[8:9], b'1')
                self.assertEqual(package[9:9+length], b'import sys; print(len(sys.stdin.buffer.read()))')
                self.assertEqual(package[9+length:], payload)
                return b""
            return b"done"

        with patch.object(provider, "run", side_effect=run):
            self.assertEqual(provider.ssh("import sys; print(len(sys.stdin.buffer.read()))", payload), b"done")
        self.assertEqual([call[1] for call in calls], ["ssh", "scp", "ssh"])
        self.assertIn("--tunnel-through-iap", calls[1])
        self.assertIn("--strict-host-key-checking=yes", calls[1])
        self.assertFalse(Path(calls[1][2]).exists())
        command = next(arg for arg in calls[2] if arg.startswith("--command="))
        encoded = command.split("b64decode('", 1)[1].split("'", 1)[0]
        wrapper = base64.b64decode(encoded).decode()
        self.assertIn('hashlib.sha256(d).hexdigest()', wrapper)
        self.assertIn("os.unlink(p)", wrapper)
        self.assertNotIn(payload.hex(), command)

    def test_all_ssh_commands_stay_short_with_large_code_and_binary_data(self):
        with patch.object(installer.shutil, "which", return_value="gcloud"):
            provider = installer.GoogleProvider("fixture@example.test", "fixture-project", "0123456789ab", self.root / "fixture-key")
        calls = []
        def run(*args, **kwargs):
            calls.append(args)
            if args[:2] == ('compute', 'scp'):
                package = Path(args[2]).read_bytes()
                length = int.from_bytes(package[:8], 'big')
                self.assertGreater(length, 100000)
                self.assertEqual(package[8:9], b'1')
                self.assertEqual(package[9+length:], b'fixture-input')
            return b'ok'
        with patch.object(provider, 'run', side_effect=run):
            self.assertEqual(provider.ssh('#' + 'x' * 120000, b'fixture-input'), b'ok')
        commands = [next(arg for arg in call if arg.startswith('--command=')) for call in calls if call[:2] == ('compute', 'ssh')]
        self.assertEqual(len(commands), 2)
        self.assertTrue(all(len(subprocess.list2cmdline(list(call))) < 8000 for call in calls))

    def test_actual_windows_ssh_command_guard_checks_the_entire_command(self):
        with patch.object(installer.shutil, 'which', return_value='gcloud'):
            provider = installer.GoogleProvider('fixture@example.test', 'fixture-project', '0123456789ab', self.root / 'fixture-key')
        with patch.object(installer.subprocess, 'run') as run:
            with self.assertRaises(installer.InstallError):
                provider.run('compute', 'ssh', provider.name, '--command=' + 'x' * 9000)
            run.assert_not_called()

    def test_ssh_without_input_transfers_code_and_cleans_up_after_failure(self):
        with patch.object(installer.shutil, 'which', return_value='gcloud'):
            provider = installer.GoogleProvider('fixture@example.test', 'fixture-project', '0123456789ab', self.root / 'fixture-key')
        calls, paths = [], []
        def run(*args, **kwargs):
            calls.append(args)
            if args[:2] == ('compute', 'scp'):
                paths.append(Path(args[2]))
                package = paths[-1].read_bytes()
                self.assertEqual(package[8:9], b'0')
                self.assertEqual(package[9:], b'raise RuntimeError()')
            if len([call for call in calls if call[:2] == ('compute', 'ssh')]) == 2:
                raise installer.InstallError('fixture remote failure')
            return b''
        with patch.object(provider, 'run', side_effect=run):
            with self.assertRaises(installer.InstallError):
                provider.ssh('raise RuntimeError()')
        self.assertEqual([call[1] for call in calls], ['ssh', 'scp', 'ssh', 'ssh'])
        self.assertFalse(paths[0].exists())
        cleanup = next(arg for arg in calls[-1] if arg.startswith('--command='))
        encoded = cleanup.split("b64decode('", 1)[1].split("'", 1)[0]
        self.assertIn('os.unlink(p)', base64.b64decode(encoded).decode())

    def test_checkpoint_services_and_updater_use_only_short_ssh_commands(self):
        import momso_anywhere_vm_services
        import momso_anywhere_update_install
        self.update_install.stop()
        with patch.object(installer.shutil, 'which', return_value='gcloud'):
            provider = installer.GoogleProvider('fixture@example.test', 'fixture-project', '0123456789ab', self.root / 'fixture-key')
        commands, packages = [], []
        def run(*args, **kwargs):
            if args[:2] == ('compute', 'ssh'):
                commands.append(args)
            if args[:2] == ('compute', 'scp'):
                packages.append(Path(args[2]).read_bytes())
            return b'momso_checkpoint_ok'
        public = self.root / 'public.pem'; public.write_bytes(b'fixture public key')
        with patch.object(provider, 'run', side_effect=run), patch.object(provider, 'health'):
            provider.upgrade_checkpoint('restore')
            momso_anywhere_vm_services.install_scripts(provider, 'fixture-secret', False, {'bucket': 'fixture', 'key': 'fixture'})
            momso_anywhere_update_install.install(provider, 'development', public, 'https://fixture.invalid/release.json')
        self.assertEqual(len(packages), 3)
        self.assertEqual(len(commands), 6)
        self.assertTrue(all(len(subprocess.list2cmdline(list(call))) < 8000 for call in commands))
        self.assertTrue(any(int.from_bytes(package[:8], 'big') > 8000 for package in packages))

    def test_ssh_failed_scp_removes_local_temporary_file(self):
        with patch.object(installer.shutil, "which", return_value="gcloud"):
            provider = installer.GoogleProvider("fixture@example.test", "fixture-project", "0123456789ab", self.root / "fixture-key")
        paths = []

        def fail(*args, **kwargs):
            if args[:2] == ("compute", "scp"):
                paths.append(Path(args[2]))
                raise installer.InstallError("fixture transfer failure")
            return b""

        with patch.object(provider, "run", side_effect=fail):
            with self.assertRaises(installer.InstallError):
                provider.ssh("print('fixture')", b"synthetic data")
        self.assertFalse(paths[0].exists())

    def test_install_payload_is_valid_and_token_is_generated_remotely(self):
        with patch.object(installer.shutil, "which", return_value="gcloud"):
            provider = installer.GoogleProvider("fixture@example.test", "fixture-project", "0123456789ab", self.root / "fixture-key")
        with patch.object(provider, "ssh", return_value=b"momso_checkpoint_ok") as ssh:
            provider.install(self.artifact, self.digest)
            self.assertEqual(ssh.call_count, 2)
            transfer = ssh.call_args_list[0].args[0]
            compile(transfer, "transfer", "exec")
            self.assertEqual(ssh.call_args_list[0].args[1], self.artifact.read_bytes())
            wrapper = ast.parse(ssh.call_args_list[1].args[0])
            encoded = next(node.value.value for node in wrapper.body if isinstance(node, ast.Assign) and isinstance(node.targets[0], ast.Name) and node.targets[0].id == "p")
            payload = base64.b64decode(encoded).decode()
            parsed = ast.parse(payload)
            unit = next(node.value.value for node in parsed.body if isinstance(node, ast.Assign) and isinstance(node.targets[0], ast.Name) and node.targets[0].id == "unit")
            self.assertIn("\nExecStart=/opt/momso/momso-server\n", unit)
            self.assertIn("secrets.token_urlsafe(32)", payload)
            self.assertIn(self.digest, payload)

    def test_web_archive_accepts_root_directory_on_legacy_python(self):
        content = io.BytesIO()
        with tarfile.open(fileobj=content, mode="w:gz") as archive:
            root = tarfile.TarInfo("./")
            root.type = tarfile.DIRTYPE
            archive.addfile(root)
            page = tarfile.TarInfo("./index.html")
            page.size = len(b"fixture")
            archive.addfile(page, io.BytesIO(b"fixture"))
        data = content.getvalue()
        source = self.root / "web.tar.gz"
        source.write_bytes(data)
        with patch.object(installer.shutil, "which", return_value="gcloud"):
            provider = installer.GoogleProvider("fixture@example.test", "fixture-project", "0123456789ab", self.root / "fixture-key")
        with patch.object(provider, "ssh", return_value=b"momso_checkpoint_ok") as ssh:
            provider.install_web(source, hashlib.sha256(data).hexdigest())
        wrapper = ast.parse(ssh.call_args.args[0])
        encoded = next(node.args[0].value for node in ast.walk(wrapper) if isinstance(node, ast.Call) and isinstance(node.func, ast.Attribute) and node.func.attr == "b64decode")
        parsed = ast.parse(base64.b64decode(encoded).decode())
        block = next(node for node in parsed.body if isinstance(node, ast.Try)).body[0]
        extracted = self.root / "extracted"
        extracted.mkdir()

        class LegacyTar:
            def __init__(self, **kwargs): self.tar = tarfile.open(**kwargs)
            def __enter__(self): return self
            def __exit__(self, *args): self.tar.close()
            def getmembers(self): return self.tar.getmembers()
            def extractall(self, path, members): self.tar.extractall(path, members=members)

        env = {"tarfile": SimpleNamespace(open=lambda **kwargs: LegacyTar(**kwargs)), "io": io,
               "pathlib": pathlib, "data": data, "tmp": str(extracted)}
        exec(compile(ast.Module(body=[block], type_ignores=[]), "fixture", "exec"), env)
        self.assertEqual((extracted / "index.html").read_bytes(), b"fixture")
        for name, kind in (("../escape", tarfile.REGTYPE), ("/absolute", tarfile.REGTYPE),
                           ("link", tarfile.SYMTYPE), ("hardlink", tarfile.LNKTYPE)):
            with self.subTest(name=name):
                content = io.BytesIO()
                with tarfile.open(fileobj=content, mode="w:gz") as archive:
                    entry = tarfile.TarInfo(name)
                    entry.type = kind
                    if kind in (tarfile.SYMTYPE, tarfile.LNKTYPE): entry.linkname = "outside"
                    archive.addfile(entry)
                env["data"] = content.getvalue()
                with self.assertRaises(AssertionError):
                    exec(compile(ast.Module(body=[block], type_ignores=[]), "fixture", "exec"), env)

    def test_google_provider_resumes_partial_resources_and_cleans_up(self):
        with patch.object(installer.shutil, "which", return_value="gcloud"):
            provider = installer.GoogleProvider("fixture@example.test", "fixture-project", self.state["id"], self.root / "fixture-key")
        cloud = SimulatedCloud(provider)
        cloud.fail_vm_once = True
        with patch.object(provider, "run", side_effect=cloud):
            with self.assertRaises(installer.InstallError):
                installer.deploy(provider, self.artifact, self.digest, self.path, self.state)
            self.assertEqual(len(cloud.resources), 4)
            installer.deploy(provider, self.artifact, self.digest, self.path, self.state)
            self.assertEqual(len(cloud.resources), 5)
            self.assertEqual(len(cloud.creates), len(set(cloud.creates)))
            provider.destroy()
            self.assertFalse(cloud.resources)
            self.assertFalse(cloud.nats)

    def test_key_cleanup_only_removes_owned_key_files(self):
        directory = installer.key_directory(self.path, self.state["id"], True)
        (directory / "ssh").write_text("fixture", encoding="ascii")
        (directory / "keep").write_text("other", encoding="ascii")
        installer.cleanup_keys(directory, self.state["id"])
        self.assertFalse((directory / "ssh").exists())
        self.assertTrue((directory / "keep").exists())
        self.assertTrue((directory / "owner").exists())

    def test_destroy_preserves_vm_with_an_extra_disk(self):
        with patch.object(installer.shutil, "which", return_value="gcloud"):
            provider = installer.GoogleProvider("fixture@example.test", "fixture-project", self.state["id"], self.root / "fixture-key")
        cloud = SimulatedCloud(provider)
        with patch.object(provider, "run", side_effect=cloud):
            provider.prepare_network()
            provider.prepare_vm()
            cloud.resources[("instances", provider.name)]["disks"].append({"boot": False, "source": "/disks/unrelated"})
            with self.assertRaises(installer.InstallError):
                provider.destroy()
            self.assertIn(("instances", provider.name), cloud.resources)

    def test_destroy_resumes_after_serverless_network_release_delay(self):
        with patch.object(installer.shutil, "which", return_value="gcloud"):
            provider = installer.GoogleProvider("fixture@example.test", "fixture-project", self.state["id"], self.root / "fixture-key")
        cloud = SimulatedCloud(provider)
        failed = []
        def delayed(*args, **kwargs):
            if args[:4] == ("compute", "networks", "subnets", "delete") and not failed:
                failed.append(True)
                raise installer.InstallError("fixture in use")
            return cloud(*args, **kwargs)
        with patch.object(provider, "run", side_effect=delayed):
            provider.prepare_network()
            provider.prepare_vm()
            with self.assertRaises(installer.PendingNetworkCleanup) as caught:
                provider.destroy()
            self.assertIn("networks subnets:" + provider.subnet, caught.exception.resources)
            self.assertIn("networks:" + provider.network, caught.exception.resources)
            provider.destroy()
            self.assertFalse(cloud.resources)

    def test_destroy_records_pending_resources_and_clears_after_retry(self):
        provider = FakeProvider()
        attempts = []
        def destroy():
            attempts.append(1)
            if len(attempts) == 1:
                raise installer.PendingNetworkCleanup(["networks subnets:fixture-sub", "networks:fixture-net"])
        provider.destroy = destroy
        args = ["destroy", "--account", "fixture@example.test", "--project", "fixture-project", "--state", str(self.path)]
        with patch.object(installer, "GoogleProvider", return_value=provider):
            self.assertEqual(installer.main(args), 1)
            pending = json.loads(self.path.read_text(encoding="utf8"))
            self.assertEqual(pending["steps"]["destroy"], "pending")
            self.assertEqual(len(pending["destroy_pending"]), 2)
            self.assertEqual(installer.main(args), 0)
            done = json.loads(self.path.read_text(encoding="utf8"))
            self.assertEqual(done["steps"]["destroy"], "ok")
            self.assertNotIn("destroy_pending", done)

    def test_mobile_action_runs_deploy_then_connection_without_logging_token(self):
        import momso_mobile_install
        calls = []
        with patch.dict("os.environ", {"MOMSO_CLOUDFLARE_API_TOKEN": "fixture-private-token"}), patch.object(installer, "GoogleProvider", return_value=FakeProvider()), patch.object(momso_mobile_install, "Cloudflare", return_value=object()), patch.object(momso_mobile_install, "connect", side_effect=lambda *args: calls.append(args)):
            code = installer.main(["mobile", "--account", "fixture@example.test", "--project", "fixture-project", "--state", str(self.path), "--artifact", str(self.artifact), "--sha256", self.digest, "--cf-account", "a" * 32, "--cf-zone", "b" * 32, "--hostname", "mobile.example.test", "--email", "fixture@example.test"])
        self.assertEqual(code, 0)
        self.assertEqual(len(calls), 1)
        self.assertNotIn("fixture-private-token", self.path.read_text(encoding="utf-8"))

    def test_mobile_action_can_add_wake_service(self):
        import momso_mobile_install
        import momso_wake_install
        with patch.dict("os.environ", {"MOMSO_CLOUDFLARE_API_TOKEN": "fixture-private-token"}), patch.object(installer, "GoogleProvider", return_value=FakeProvider()), patch.object(momso_mobile_install, "Cloudflare", return_value=object()), patch.object(momso_mobile_install, "connect"), patch.object(momso_wake_install, "GoogleWake", return_value=object()), patch.object(momso_wake_install, "install", return_value="https://fixture.a.run.app") as install:
            code = installer.main(["mobile", "--account", "fixture@example.test", "--project", "fixture-project", "--state", str(self.path), "--artifact", str(self.artifact), "--sha256", self.digest, "--cf-account", "a" * 32, "--cf-zone", "b" * 32, "--hostname", "mobile.example.test", "--email", "fixture@example.test", "--wake-user", "user@example.test"])
        self.assertEqual(code, 0)
        self.assertEqual(install.call_args.args[-2:], ("mobile.example.test", "user@example.test"))

    def test_anywhere_installs_server_and_verified_web_before_gateway(self):
        import momso_anywhere_install
        import momso_anywhere_public_assets
        provider = FakeProvider()
        web = self.root / "web.tar.gz"
        web.write_bytes(b"fictional web bundle")
        digest = hashlib.sha256(web.read_bytes()).hexdigest()
        with patch.object(installer, "GoogleProvider", return_value=provider), patch.object(momso_anywhere_public_assets, "from_web_archive", return_value={}), patch.object(momso_anywhere_install, "install", return_value="https://fixture.a.run.app") as install:
            code = installer.main(["anywhere", "--account", "fixture@example.test", "--project", "fixture-project", "--state", str(self.path), "--artifact", str(self.artifact), "--sha256", self.digest, "--web-archive", str(web), "--web-sha256", digest, "--google-client-id", "fixture.apps.googleusercontent.com", "--owner-sub", "12345", "--login-origin", "https://login.example.test", "--gateway-image", "registry.example.test/momso/gateway@sha256:" + "a" * 64, '--release-manifest-url', 'https://example.test/release.json', '--release-public-key', str(self.public_key)])
        self.assertEqual(code, 0)
        self.assertEqual(provider.calls, ["network", "vm", "install", "health", "web"])
        self.update_grant_mock.assert_called_once_with(provider)
        self.update_install_mock.assert_called_once_with(provider, "production", self.public_key, "https://example.test/release.json")
        install.assert_called_once()

    def test_anywhere_wrong_web_digest_creates_no_resources(self):
        provider = FakeProvider()
        web = self.root / "web.tar.gz"
        web.write_bytes(b"fixture")
        with patch.object(installer, "GoogleProvider", return_value=provider):
            code = installer.main(["anywhere", "--account", "fixture@example.test", "--project", "fixture-project", "--state", str(self.path), "--artifact", str(self.artifact), "--sha256", self.digest, "--web-archive", str(web), "--web-sha256", "0" * 64, "--google-client-id", "fixture.apps.googleusercontent.com", "--owner-sub", "12345", "--login-origin", "https://login.example.test", "--gateway-image", "registry.example.test/momso/gateway@sha256:" + "a" * 64, '--release-manifest-url', 'https://example.test/release.json', '--release-public-key', str(self.public_key)])
        self.assertEqual(code, 1)
        self.assertFalse(provider.calls)

    def test_anywhere_requires_pinned_gateway_image_before_creating_resources(self):
        provider = FakeProvider()
        web = self.root / 'web.tar.gz'
        web.write_bytes(b'fixture')
        previous = self.path.read_bytes()
        args = ['anywhere', '--account', 'fixture@example.test', '--project', 'fixture-project', '--state', str(self.path), '--artifact', str(self.artifact), '--sha256', self.digest, '--web-archive', str(web), '--web-sha256', hashlib.sha256(web.read_bytes()).hexdigest(), '--google-client-id', 'fixture.apps.googleusercontent.com', '--owner-sub', '12345', '--login-origin', 'https://login.example.test']
        with patch.object(installer, 'GoogleProvider', return_value=provider):
            self.assertEqual(installer.main([*args, '--gateway-image', 'registry.example.test/gateway:latest', '--release-manifest-url', 'https://example.test/release.json', '--release-public-key', str(self.public_key)]), 1)
        self.assertFalse(provider.calls)
        self.assertEqual(self.path.read_bytes(), previous)

    def test_anywhere_accepts_cloud_run_default_login_origin(self):
        import momso_anywhere_install
        import momso_anywhere_public_assets
        web = self.root / "web.tar.gz"
        web.write_bytes(b"fictional web bundle")
        digest = hashlib.sha256(web.read_bytes()).hexdigest()
        origin = "https://fixture-123.asia-northeast3.run.app"
        with patch.object(installer, "GoogleProvider", return_value=FakeProvider()), patch.object(momso_anywhere_public_assets, "from_web_archive", return_value={}), patch.object(momso_anywhere_install, "install", return_value="https://fixture.a.run.app") as install:
            code = installer.main(["anywhere", "--account", "fixture@example.test", "--project", "fixture-project", "--state", str(self.path), "--artifact", str(self.artifact), "--sha256", self.digest, "--web-archive", str(web), "--web-sha256", digest, "--google-client-id", "fixture.apps.googleusercontent.com", "--owner-sub", "12345", "--login-origin", origin, "--gateway-image", "registry.example.test/momso/gateway@sha256:" + "a" * 64, '--release-manifest-url', 'https://example.test/release.json', '--release-public-key', str(self.public_key)])
        self.assertEqual(code, 0)
        self.assertEqual(install.call_args.args[-2], origin)

    def test_existing_anywhere_upgrade_changes_only_server_and_web(self):
        import momso_anywhere_install
        import momso_anywhere_public_assets
        web = self.root / "web.tar.gz"
        web.write_bytes(b"new web")
        web_hash = hashlib.sha256(web.read_bytes()).hexdigest()
        data = self.root / "server.sqlite3"
        data.write_bytes(b"synthetic chat data")

        class ExistingProvider:
            install_id = "0123456789ab"
            project = "fixture-project"
            name = "momso-" + install_id
            network = name + "-net"
            def __init__(self): self.calls = []
            def run(self, *args, **kwargs):
                self.calls.append(args)
                if args[:3] == ("compute", "instances", "describe"):
                    return {"labels": {"momso-install-id": self.install_id}, "status": "RUNNING", "networkInterfaces": [{"network": "/networks/" + self.network}]}
                if args[:3] == ("run", "services", "describe"):
                    return {"status": {"url": "https://fixture.a.run.app"}}
                raise AssertionError("unexpected cloud write")
            def health(self): self.calls.append(("health",))
            def ensure_private_proxy(self): self.calls.append(("private_proxy",))
            def upgrade_checkpoint(self, action): self.calls.append(("checkpoint", action))
            def upgrade_server(self, *args): self.calls.append(("upgrade_server",))
            def install_web(self, *args): self.calls.append(("install_web",))
            def upgrade_sequence(self, sequence, *, commit=False): self.calls.append(("sequence", sequence, commit))

        provider = ExistingProvider()
        state = {"anywhere": {"client_id": "client", "owner_sub": "12345", "login_origin": "https://login.example.test", "url": "https://fixture.a.run.app"}}
        with patch.object(momso_anywhere_public_assets, "from_web_archive", return_value={}), patch.object(momso_anywhere_install.GoogleAnywhere, "_objects", return_value=({}, {}, {})):
            # Empty objects cannot prove ownership.
            with self.assertRaises(installer.InstallError):
                installer.upgrade_anywhere(provider, state, self.artifact, self.digest, web, web_hash, "client", "12345", "https://login.example.test", "registry.example.test/momso/gateway@sha256:" + "a" * 64)
        with patch.object(momso_anywhere_public_assets, "from_web_archive", return_value={}), patch.object(momso_anywhere_install.GoogleAnywhere, "_objects", return_value=({"name": "account"}, {"name": "role"}, {"name": "service"})), patch.object(momso_anywhere_install.GoogleAnywhere, "upgrade_gateway") as upgrade_gateway, patch("momso_anywhere_vm_services.install") as install_services:
            url = installer.upgrade_anywhere(provider, state, self.artifact, self.digest, web, web_hash, "client", "12345", "https://login.example.test", "registry.example.test/momso/gateway@sha256:" + "a" * 64)
        install_services.assert_called_once_with(provider, allow_upgrade_gate=True)
        upgrade_gateway.assert_called_once_with(state["anywhere"]["url"], "registry.example.test/momso/gateway@sha256:" + "a" * 64)
        self.assertEqual(url, state["anywhere"]["url"])
        self.assertEqual(data.read_bytes(), b"synthetic chat data")
        self.assertEqual([x[0] for x in provider.calls].count("upgrade_server"), 1)
        self.assertEqual([x[0] for x in provider.calls].count("install_web"), 1)
        self.assertFalse(any("delete" in call or "create" in call for call in provider.calls))
        self.assertEqual([call for call in provider.calls if call[0] == "checkpoint"], [("checkpoint", "begin"), ("checkpoint", "finish")])
        self.assertEqual([call for call in provider.calls if call[0] == "private_proxy"], [("private_proxy",)])

        provider.calls.clear()
        with patch.object(momso_anywhere_public_assets, "from_web_archive", return_value={}), patch.object(momso_anywhere_install.GoogleAnywhere, "_objects", return_value=({"name": "account"}, {"name": "role"}, {"name": "service"})), patch.object(momso_anywhere_install.GoogleAnywhere, "upgrade_gateway"), patch("momso_anywhere_vm_services.install"):
            installer.upgrade_anywhere(provider, state, self.artifact, self.digest, web, web_hash, "client", "12345", "https://login.example.test", "registry.example.test/momso/gateway@sha256:" + "a" * 64, signed_sequence=3)
        self.assertEqual([call for call in provider.calls if call[0] == "sequence"], [("sequence", 3, False), ("sequence", 3, True)])

        provider.calls.clear()
        health_calls = []
        def fail_new_health():
            health_calls.append(1)
            if len(health_calls) == 1: raise installer.InstallError("fixture unhealthy")
        provider.health = fail_new_health
        with patch.object(momso_anywhere_public_assets, "from_web_archive", return_value={}), patch.object(momso_anywhere_install.GoogleAnywhere, "_objects", return_value=({"name": "account"}, {"name": "role"}, {"name": "service"})), patch.object(momso_anywhere_install.GoogleAnywhere, "upgrade_gateway") as upgrade_gateway, patch("momso_anywhere_vm_services.install") as install_services:
            with self.assertRaisesRegex(installer.InstallError, "되돌렸습니다"):
                installer.upgrade_anywhere(provider, state, self.artifact, self.digest, web, web_hash, "client", "12345", "https://login.example.test", "registry.example.test/momso/gateway@sha256:" + "a" * 64)
        self.assertEqual([call for call in provider.calls if call[0] == "checkpoint"], [("checkpoint", "begin"), ("checkpoint", "restore"), ("checkpoint", "finish")])
        self.assertEqual(len(health_calls), 2)
        upgrade_gateway.assert_not_called()
        self.assertEqual([call for call in provider.calls if call[0] == "private_proxy"], [("private_proxy",)])

        provider.calls.clear()
        provider.health = lambda: provider.calls.append(("health",))
        with patch.object(momso_anywhere_public_assets, "from_web_archive", return_value={}), patch.object(momso_anywhere_install.GoogleAnywhere, "_objects", return_value=({"name": "account"}, {"name": "role"}, {"name": "service"})), patch.object(momso_anywhere_install.GoogleAnywhere, "upgrade_gateway", side_effect=installer.InstallError("fixture gateway failure")), patch("momso_anywhere_vm_services.install"):
            with self.assertRaisesRegex(installer.InstallError, "되돌렸습니다"):
                installer.upgrade_anywhere(provider, state, self.artifact, self.digest, web, web_hash, "client", "12345", "https://login.example.test", "registry.example.test/momso/gateway@sha256:" + "a" * 64)
        self.assertEqual([call for call in provider.calls if call[0] == "checkpoint"], [("checkpoint", "begin"), ("checkpoint", "restore"), ("checkpoint", "finish")])

    def test_existing_anywhere_command_uses_upgrade_path(self):
        import momso_anywhere_public_assets
        web = self.root / "web.tar.gz"
        web.write_bytes(b"fictional web")
        digest = hashlib.sha256(web.read_bytes()).hexdigest()
        self.state["anywhere"] = {"client_id": "fixture.apps.googleusercontent.com", "owner_sub": "12345", "login_origin": "https://login.example.test", "url": "https://fixture.a.run.app"}
        installer.save_state(self.path, self.state)
        with patch.object(installer, "GoogleProvider", return_value=FakeProvider()), patch.object(momso_anywhere_public_assets, "from_web_archive", return_value={}), patch.object(installer, "deploy") as deploy, patch.object(installer, "upgrade_anywhere", return_value="https://fixture.a.run.app") as upgrade:
            result = installer.main(["anywhere", "--account", "fixture@example.test", "--project", "fixture-project", "--state", str(self.path), "--artifact", str(self.artifact), "--sha256", self.digest, "--web-archive", str(web), "--web-sha256", digest, "--google-client-id", "fixture.apps.googleusercontent.com", "--owner-sub", "12345", "--login-origin", "https://login.example.test", "--gateway-image", "registry.example.test/momso/gateway@sha256:" + "a" * 64, '--release-manifest-url', 'https://example.test/release.json', '--release-public-key', str(self.public_key)])
        self.assertEqual(result, 0)
        upgrade.assert_called_once()
        deploy.assert_not_called()

    def test_existing_install_passes_signed_ghcr_image_to_upgrade(self):
        import momso_anywhere_public_assets
        image = 'ghcr.io/example/momso-gateway@sha256:' + 'a' * 64
        web = self.root / 'web.tar.gz'
        web.write_bytes(b'fictional web')
        digest = hashlib.sha256(web.read_bytes()).hexdigest()
        self.state['anywhere'] = {'client_id': 'fixture.apps.googleusercontent.com', 'owner_sub': '12345', 'login_origin': 'https://login.example.test', 'url': 'https://fixture.a.run.app'}
        installer.save_state(self.path, self.state)
        release = {'gatewayImage': image, 'bundleUrl': 'https://example.test/bundle', 'bundleSha256': 'b' * 64, 'sequence': 2}
        with patch.object(momso_release_manifest, 'release', return_value=release), patch.object(installer, 'GoogleProvider', return_value=FakeProvider()), patch.object(momso_anywhere_public_assets, 'from_web_archive', return_value={}), patch.object(installer, 'upgrade_anywhere', return_value='https://fixture.a.run.app') as upgrade:
            result = installer.main(['anywhere', '--account', 'fixture@example.test', '--project', 'fixture-project', '--state', str(self.path), '--artifact', str(self.artifact), '--sha256', self.digest, '--web-archive', str(web), '--web-sha256', digest, '--google-client-id', 'fixture.apps.googleusercontent.com', '--owner-sub', '12345', '--login-origin', 'https://login.example.test', '--gateway-image', image, '--release-manifest-url', 'https://example.test/release.json', '--release-public-key', str(self.public_key)])
        self.assertEqual(result, 0)
        self.assertIn(image, upgrade.call_args.args)

    def test_first_install_prepares_git_before_server_start(self):
        with patch.object(installer.shutil, "which", return_value="gcloud"):
            provider = installer.GoogleProvider("fixture@example.test", "fixture-project", "0123456789ab", self.root / "fixture-key")
        with patch.object(provider, "ssh", return_value=b"") as ssh:
            provider.install(self.artifact, self.digest)
        tree = ast.parse(ssh.call_args_list[1].args[0])
        encoded = next(node.value.value for node in tree.body if isinstance(node, ast.Assign) and isinstance(node.targets[0], ast.Name) and node.targets[0].id == "p")
        payload = base64.b64decode(encoded).decode()
        self.assertLess(payload.index("('git','ffmpeg')"), payload.index("run('systemctl','enable'"))

    def test_server_upgrade_payload_does_not_touch_data_or_configuration(self):
        with patch.object(installer.shutil, "which", return_value="gcloud"):
            provider = installer.GoogleProvider("fixture@example.test", "fixture-project", "0123456789ab", self.root / "fixture-key")
        with patch.object(provider, "ssh", return_value=b"momso_checkpoint_ok") as ssh:
            provider.upgrade_server(self.artifact, self.digest)
        tree = ast.parse(ssh.call_args.args[0])
        encoded = next(node.args[0].value for node in ast.walk(tree) if isinstance(node, ast.Call) and isinstance(node.func, ast.Attribute) and node.func.attr == "b64decode")
        payload = base64.b64decode(encoded).decode()
        ast.parse(payload, feature_version=(3, 11))
        self.assertNotIn("/var/lib/momso", payload)
        self.assertNotIn("/etc/momso", payload)
        self.assertNotIn("useradd", payload)
        self.assertEqual(ssh.call_args.args[1], self.artifact.read_bytes())

    def test_server_upgrade_updates_active_release_instead_of_old_flat_file(self):
        import io
        import sys
        with patch.object(installer.shutil, "which", return_value="gcloud"):
            provider = installer.GoogleProvider("fixture@example.test", "fixture-project", "0123456789ab", self.root / "fixture-key")
        flat = self.root / "flat"
        release = flat / "releases" / "version-1"
        release.mkdir(parents=True)
        (flat / "momso-server").write_bytes(b"old flat")
        (release / "momso-server").write_bytes(b"old active")
        (flat / "releases" / ".momso-update-active").write_text("momso-update-v1\n")
        (flat / "releases" / "current").symlink_to(release, target_is_directory=True)
        with patch.object(provider, "ssh", return_value=b"") as ssh:
            provider.upgrade_server(self.artifact, self.digest)
        tree = ast.parse(ssh.call_args.args[0])
        encoded = next(node.args[0].value for node in ast.walk(tree) if isinstance(node, ast.Call) and isinstance(node.func, ast.Attribute) and node.func.attr == "b64decode")
        payload = base64.b64decode(encoded).decode().replace("root='/opt/momso'", "root=" + repr(str(flat)))
        with patch.object(sys, "stdin", SimpleNamespace(buffer=io.BytesIO(self.artifact.read_bytes()))), patch.object(subprocess, "run"):
            exec(compile(payload, "fixture", "exec"), {})
        self.assertEqual((release / "momso-server").read_bytes(), self.artifact.read_bytes())
        self.assertEqual((flat / "momso-server").read_bytes(), b"old flat")

    def test_upgrade_checkpoint_restores_server_and_screen_without_touching_other_files(self):
        with patch.object(installer.shutil, "which", return_value="gcloud"):
            provider = installer.GoogleProvider("fixture@example.test", "fixture-project", "0123456789ab", self.root / "fixture-key")
        root = self.root / "opt"
        root.mkdir()
        web = root / "web"
        web.mkdir()
        (web / ".momso-owned").write_text("momso")
        (web / "index.html").write_text("old screen")
        (root / "momso-server").write_text("old server")
        (root / "other-failed-file").write_text("preserve")
        config = self.root / "server.env"
        data = self.root / "data"
        data.mkdir()
        (data / "database.sqlite").write_text("old database")
        (data / "vault.age").write_text("old vault")
        (data / "archive").mkdir()
        (data / "archive" / "lesson.md").write_text("old archive")
        config.write_text("MOMSO_SERVER_TOKEN=fixture\nMOMSO_SERVER_DATA_DIR=" + str(data) + "\n")
        readiness = [False]
        def response(req, timeout):
            from io import BytesIO
            if req.full_url.endswith("/activity"):
                return BytesIO(json.dumps({"maintenance": False, "quiescent": True, "maintenanceReady": readiness[0], "maintenanceReason": None if readiness[0] else "backup_not_integrated"}).encode())
            return BytesIO(b'{"maintenance":true}' if req.full_url.endswith("/maintenance") else b'{"maintenance":false}')
        def run(action):
            with patch.object(provider, "ssh", return_value=b"momso_checkpoint_ok") as ssh:
                provider.upgrade_checkpoint(action)
            tree = ast.parse(ssh.call_args.args[0])
            encoded = next(node.args[0].value for node in ast.walk(tree) if isinstance(node, ast.Call) and isinstance(node.func, ast.Attribute) and node.func.attr == "b64decode")
            payload = base64.b64decode(encoded).decode().replace("root='/opt/momso'", "root=" + repr(str(root))).replace("env='/etc/momso-server.env'", "env=" + repr(str(config)))
            ast.parse(payload, feature_version=(3, 11))
            import sys
            fcntl_stub = SimpleNamespace(LOCK_EX=2, LOCK_NB=4, flock=lambda *_: None)
            with patch.object(subprocess, "run", return_value=SimpleNamespace(returncode=0)), patch("urllib.request.urlopen", side_effect=response), patch.dict(sys.modules, {"fcntl": fcntl_stub}), patch.object(os, "O_NOFOLLOW", 0, create=True), patch.object(os, "chown", lambda *_, **__: None, create=True), patch.object(os, "fchown", lambda *_: None, create=True):
                context = {}
                try:
                    exec(compile(payload, "fixture", "exec"), context)
                finally:
                    if "lock" in context:
                        os.close(context["lock"])
        # Backup status does not block an idle update.
        run("begin")
        (root / "momso-server").write_text("bad server")
        (web / "index.html").write_text("bad screen")
        (data / "database.sqlite").write_text("new migrated database")
        (data / "vault.age").write_text("new vault")
        (data / "archive" / "lesson.md").write_text("new archive")
        run("restore")
        self.assertEqual((root / "momso-server").read_text(), "old server")
        self.assertEqual((web / "index.html").read_text(), "old screen")
        self.assertEqual((root / "other-failed-file").read_text(), "preserve")
        self.assertEqual((data / "database.sqlite").read_text(), "old database")
        self.assertEqual((data / "vault.age").read_text(), "old vault")
        self.assertEqual((data / "archive" / "lesson.md").read_text(), "old archive")
        run("finish")
        self.assertFalse((root / ".momso-upgrade-backup").exists())


if __name__ == "__main__":
    unittest.main()


class InstallerFilesTests(unittest.TestCase):
    def test_missing_installer_components_have_friendly_error(self):
        import tempfile
        from pathlib import Path
        import momso_server_install as installer
        with tempfile.TemporaryDirectory() as directory:
            with self.assertRaisesRegex(installer.InstallError, "전체 설치기 묶음"):
                installer.validate_installer_files(directory)
            for name in installer.INSTALLER_FILES:
                path = Path(directory) / name
                path.parent.mkdir(parents=True, exist_ok=True)
                path.write_text('fixture', encoding='utf-8')
            installer.validate_installer_files(directory)
