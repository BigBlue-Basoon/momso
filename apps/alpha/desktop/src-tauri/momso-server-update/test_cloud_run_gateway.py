import json
import subprocess
import unittest
from cloud_run_gateway import CloudRunGateway, GatewayError


class FakeCloudRun:
    def __init__(self): self.calls = []; self.traffic = {"service-old": 100}; self.latest = "service-old"
    def __call__(self, command, **kwargs):
        self.calls.append(command)
        if "describe" in command:
            data = {"status": {"traffic": [{"revisionName": key, "percent": value} for key, value in self.traffic.items()], "latestReadyRevisionName": self.latest, "conditions": [{"type":"Ready", "status":"True"}]}}
            return subprocess.CompletedProcess(command, 0, json.dumps(data).encode(), b"")
        if "deploy" in command: self.latest = "service-new"
        if "update-traffic" in command:
            spec = command[command.index("--to-revisions") + 1]
            self.traffic = {name: int(value) for name, value in (part.split("=") for part in spec.split(","))}
        return subprocess.CompletedProcess(command, 0, b"", b"")


class CloudRunTests(unittest.TestCase):
    def test_pinned_deploy_and_restore_previous_revision(self):
        fake = FakeCloudRun(); gateway = CloudRunGateway("fixture-project", "asia-northeast3", "fixture-gateway", runner=fake)
        original = gateway.current()
        gateway.activate("registry.example/momso@sha256:" + "1" * 64)
        self.assertEqual(fake.traffic, {"service-new": 100})
        self.assertTrue(gateway.healthy())
        gateway.activate(original)
        self.assertEqual(fake.traffic, {"service-old": 100})
        self.assertTrue(any("--no-traffic" in command for command in fake.calls))
    def test_unpinned_image_is_rejected(self):
        gateway = CloudRunGateway("fixture-project", "asia-northeast3", "fixture-gateway", runner=FakeCloudRun())
        with self.assertRaises(GatewayError): gateway.activate("registry.example/momso:latest")


if __name__ == "__main__": unittest.main()
