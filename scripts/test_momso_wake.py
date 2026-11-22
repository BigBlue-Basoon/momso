import importlib.util
import http.client
import http.server
import json
import tempfile
import threading
import unittest
from pathlib import Path
from unittest.mock import patch

import momso_wake_install as installer

spec = importlib.util.spec_from_file_location("wake_service", Path(__file__).with_name("momso_wake") / "main.py")
service = importlib.util.module_from_spec(spec)
spec.loader.exec_module(service)


class Provider:
    project = "fixture-project"
    install_id = "0123456789ab"
    marker = "momso-install-0123456789ab"
    name = "momso-0123456789ab"

    def __init__(self):
        self.account = self.role = self.service = None
        self.calls = []
        self.fail_deploy = False

    def run(self, *args, parse=False):
        self.calls.append(args)
        if args[:3] == ("iam", "service-accounts", "list"): return [self.account] if self.account else []
        if args[:3] == ("iam", "service-accounts", "describe"): return self.account
        if args[:3] == ("iam", "roles", "list"): return [self.role] if self.role else []
        if args[:3] == ("iam", "roles", "describe"): return self.role
        if args[:3] == ("run", "services", "list"): return [self.service] if self.service else []
        if args[:3] == ("iam", "service-accounts", "create"):
            self.account = {"email": "mw-0123456789ab" + "@" + "fixture-project.iam.gserviceaccount.com", "description": self.marker}
        elif args[:3] == ("iam", "roles", "create"):
            self.role = {"name": "projects/fixture-project/roles/momsoWake0123456789ab", "description": self.marker, "includedPermissions": ["compute.instances.get", "compute.instances.start"]}
        elif args[:2] == ("run", "deploy"):
            if self.fail_deploy:
                self.fail_deploy = False
                raise installer.InstallError("fixture interruption")
            self.service = {"metadata": {"name": "momso-wake-0123456789ab", "labels": {"momso-install-id": self.install_id}}}
        elif args[:3] == ("projects", "describe", self.project): return {"projectNumber": "123456789"}
        elif args[:3] == ("run", "services", "describe"): return {"status": {"url": "https://momso-wake-fixture.a.run.app"}}
        elif args[:3] == ("run", "services", "delete"): self.service = None
        elif args[:3] == ("iam", "service-accounts", "delete"): self.account = None
        elif args[:3] == ("iam", "roles", "delete"): self.role = None
        return b""


class WakeTests(unittest.TestCase):
    def test_install_retries_and_destroy_rechecks_ownership(self):
        p = Provider()
        wake = installer.GoogleWake(p)
        with tempfile.TemporaryDirectory() as directory:
            state = {}
            path = Path(directory) / "state.json"
            p.fail_deploy = True
            with self.assertRaises(installer.InstallError):
                installer.install(wake, path, state, "mobile.example.test", "user@example.test")
            url = installer.install(wake, path, state, "mobile.example.test", "user@example.test")
            self.assertTrue(url.endswith(".run.app"))
            self.assertEqual(sum(c[:3] == ("iam", "roles", "create") for c in p.calls), 1)
            deploy = next(c for c in p.calls if c[:2] == ("run", "deploy"))
            self.assertIn("--iap", deploy)
            self.assertIn("--no-allow-unauthenticated", deploy)
            p.service["metadata"]["labels"] = {}
            with self.assertRaises(installer.InstallError): wake.destroy()
            self.assertIsNotNone(p.account)
            p.service["metadata"]["labels"] = {"momso-install-id": p.install_id}
            wake.destroy()
            self.assertIsNone(p.account)
            self.assertIsNone(p.role)
            self.assertIsNone(p.service)

    def test_only_configured_vm_is_started(self):
        values = {"MOMSO_PROJECT": "fixture-project", "MOMSO_ZONE": "test-zone", "MOMSO_INSTANCE": "fixture-vm"}
        calls = []
        def api(url, **kwargs):
            calls.append((url, kwargs))
            if kwargs.get("metadata"): return {"access_token": "fixture-token"}
            if kwargs.get("method") == "POST": return {}
            return {"status": "TERMINATED"}
        with patch.dict("os.environ", values), patch.object(service, "api_request", side_effect=api):
            self.assertEqual(service.wake(), "STARTING")
        self.assertEqual(calls[-1][0], "https://compute.googleapis.com/compute/v1/projects/fixture-project/zones/test-zone/instances/fixture-vm/start")
        self.assertNotIn("fixture-token", json.dumps([url for url, _ in calls]))

    def test_running_vm_is_not_started_again(self):
        values = {"MOMSO_PROJECT": "fixture-project", "MOMSO_ZONE": "test-zone", "MOMSO_INSTANCE": "fixture-vm"}
        with patch.dict("os.environ", values), patch.object(service, "api_request", side_effect=[{"access_token": "fixture"}, {"status": "RUNNING"}]) as api:
            self.assertEqual(service.wake(), "RUNNING")
            self.assertEqual(api.call_count, 2)

    def test_cross_origin_request_cannot_start_vm(self):
        server = http.server.ThreadingHTTPServer(("127.0.0.1", 0), service.Handler)
        thread = threading.Thread(target=server.serve_forever, daemon=True)
        thread.start()
        try:
            with patch.object(service, "wake") as wake:
                client = http.client.HTTPConnection("127.0.0.1", server.server_address[1])
                client.request("POST", "/wake", headers={"Host": "fixture.run.app", "Origin": "https://other.example.test"})
                response = client.getresponse()
                self.assertEqual(response.status, 403)
                response.read()
                client.close()
                wake.assert_not_called()
        finally:
            server.shutdown()
            server.server_close()
            thread.join(2)


if __name__ == "__main__":
    unittest.main()
