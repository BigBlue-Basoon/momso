import json
import subprocess
import unittest
from cloud_run_gateway import CloudRunGateway, GatewayError


class FakeCloudRun:
    def __init__(self): self.calls = []; self.traffic = {"service-old": 100}; self.latest = "service-old"; self.image = None; self.ignore_traffic = False
    def __call__(self, command, **kwargs):
        self.calls.append(command)
        if "revisions" in command and "describe" in command:
            data = {"status": {"imageDigest": self.image, "conditions": [{"type": "Ready", "status": "True", "reason": "Retired"}, {"type": "Active", "status": "False"}]}}
            return subprocess.CompletedProcess(command, 0, json.dumps(data).encode(), b"")
        if "describe" in command:
            data = {"status": {"url": "https://fixture.run.app", "traffic": [{"revisionName": key, "percent": value} for key, value in self.traffic.items()] + [{"revisionName": f"service-unused-{i}", "percent": 0} for i in range(6)], "latestReadyRevisionName": "service-old", "latestCreatedRevisionName": self.latest, "conditions": [{"type":"Ready", "status":"True"}]}}
            return subprocess.CompletedProcess(command, 0, json.dumps(data).encode(), b"")
        if "deploy" in command: self.latest = "service-new"; self.image = command[command.index('--image') + 1]
        if "update-traffic" in command:
            spec = command[command.index("--to-revisions") + 1]
            if not self.ignore_traffic: self.traffic = {name: int(value) for name, value in (part.split("=") for part in spec.split(","))}
        return subprocess.CompletedProcess(command, 0, b"", b"")


class CloudRunTests(unittest.TestCase):
    def test_first_deploy_does_not_use_no_traffic(self):
        fake = FakeCloudRun()
        def fresh(command, **kwargs):
            if 'deploy' in command and '--no-traffic' in command:
                return subprocess.CompletedProcess(command, 1, b'', b'--no-traffic not supported when creating a new service')
            if 'deploy' in command:
                fake.traffic = {'service-new': 100}
            return fake(command, **kwargs)
        gateway = CloudRunGateway('fixture-project', 'asia-northeast3', 'fixture-gateway', runner=fresh, probe=lambda _: None)
        gateway.activate('ghcr.io/example/gateway@sha256:' + '2' * 64, first_deploy=True)
        self.assertEqual(fake.traffic, {'service-new': 100})
        self.assertFalse(any('--no-traffic' in call for call in fake.calls if 'deploy' in call))
        self.assertTrue(any('--max=1' in call for call in fake.calls if 'deploy' in call))

    def test_stale_latest_created_revision_cannot_be_promoted(self):
        fake = FakeCloudRun()
        def runner(command, **kwargs):
            if 'deploy' in command:
                return subprocess.CompletedProcess(command, 0, b'', b'')
            return fake(command, **kwargs)
        gateway = CloudRunGateway('fixture-project', 'asia-northeast3', 'fixture-gateway', runner=runner, probe=lambda _: None)
        with self.assertRaisesRegex(GatewayError, '^gateway_not_ready$'):
            gateway.activate('ghcr.io/example/momso-gateway@sha256:' + '2' * 64)
        self.assertEqual(fake.traffic, {'service-old': 100})

    def test_wrong_resolved_digest_never_moves_traffic(self):
        fake = FakeCloudRun()
        runner = fake.__call__
        def wrong_digest(command, **kwargs):
            if 'revisions' in command and 'describe' in command:
                return subprocess.CompletedProcess(command, 0, json.dumps({'status': {'imageDigest': 'sha256:' + '3' * 64, 'conditions': [{'type': 'Ready', 'status': 'True'}]}}).encode(), b'')
            return runner(command, **kwargs)
        gateway = CloudRunGateway('fixture-project', 'asia-northeast3', 'fixture-gateway', runner=wrong_digest)
        with self.assertRaisesRegex(GatewayError, '^gateway_image_mismatch$'):
            gateway.activate('ghcr.io/example/momso-gateway@sha256:' + '2' * 64)
        self.assertEqual(fake.traffic, {'service-old': 100})
        self.assertFalse(any('update-traffic' in command for command in fake.calls))

    def test_cloud_run_cached_repository_keeps_digest(self):
        fake = FakeCloudRun()
        def cached(command, **kwargs):
            result = fake(command, **kwargs)
            if 'revisions' in command and 'describe' in command:
                value = json.loads(result.stdout)
                value['status']['imageDigest'] = 'cache.region-docker.pkg.dev/' + value['status']['imageDigest']
                return subprocess.CompletedProcess(command, 0, json.dumps(value).encode(), b'')
            return result
        gateway = CloudRunGateway('fixture-project', 'asia-northeast3', 'fixture-gateway', runner=cached, probe=lambda _: None)
        gateway.activate('ghcr.io/example/gateway@sha256:' + '2' * 64)
        self.assertEqual(fake.traffic, {'service-new': 100})

    def test_stale_pinned_traffic_is_not_reported_as_success(self):
        fake = FakeCloudRun(); fake.ignore_traffic = True
        gateway = CloudRunGateway("fixture-project", "asia-northeast3", "fixture-gateway", runner=fake)
        with self.assertRaisesRegex(GatewayError, '^gateway_traffic_mismatch$'):
            gateway.activate("ghcr.io/example/momso-gateway@sha256:" + "2" * 64)
        self.assertEqual(fake.traffic, {"service-old": 100})
    def test_public_ghcr_migration_and_rollback_keep_signed_digest(self):
        fake = FakeCloudRun(); gateway = CloudRunGateway("fixture-project", "asia-northeast3", "fixture-gateway", runner=fake, probe=lambda _: None)
        image = "ghcr.io/example/momso-gateway@sha256:" + "2" * 64
        original = gateway.current()
        self.assertFalse(gateway.preflight(image))
        self.assertFalse(any("artifacts" in command for command in fake.calls))
        gateway.activate(image)
        self.assertTrue(any(image in command for command in fake.calls))
        gateway.activate(original)
        self.assertEqual(fake.traffic, {"service-old": 100})

    def test_pinned_deploy_and_restore_previous_revision(self):
        fake = FakeCloudRun(); gateway = CloudRunGateway("fixture-project", "asia-northeast3", "fixture-gateway", runner=fake, probe=lambda _: None)
        original = gateway.current()
        gateway.activate("registry.example/momso@sha256:" + "1" * 64)
        self.assertEqual(fake.traffic, {"service-new": 100})
        self.assertTrue(gateway.healthy())
        gateway.activate(original)
        self.assertEqual(fake.traffic, {"service-old": 100})
        self.assertTrue(any("--no-traffic" in command for command in fake.calls))
        self.assertTrue(any("--max=1" in command for command in fake.calls if "deploy" in command))
    def test_unpinned_image_is_rejected(self):
        gateway = CloudRunGateway("fixture-project", "asia-northeast3", "fixture-gateway", runner=FakeCloudRun())
        with self.assertRaises(GatewayError): gateway.activate("registry.example/momso:latest")

    def test_download_permission_is_reported_without_provider_text(self):
        def denied(command, **kwargs):
            return subprocess.CompletedProcess(command, 1, b"", b"secret fixture: artifactregistry.repositories.downloadArtifacts denied")
        gateway = CloudRunGateway("fixture-project", "asia-northeast3", "fixture-gateway", runner=denied)
        with self.assertRaisesRegex(GatewayError, "^gateway_image_permission$"):
            gateway.preflight("registry.example/momso@sha256:" + "1" * 64)
        with self.assertRaisesRegex(GatewayError, "^gateway_image_permission$"):
            gateway.activate("registry.example/momso@sha256:" + "1" * 64)

    def test_inconclusive_describe_does_not_block(self):
        def denied(command, **kwargs): return subprocess.CompletedProcess(command, 1, b"", b"different permission")
        gateway = CloudRunGateway("fixture-project", "asia-northeast3", "fixture-gateway", runner=denied)
        self.assertFalse(gateway.preflight("registry.example/momso@sha256:" + "1" * 64))


if __name__ == "__main__": unittest.main()
