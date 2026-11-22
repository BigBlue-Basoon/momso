import unittest
from unittest.mock import patch

import worker


class WorkerTests(unittest.TestCase):
    def test_wires_channel_admission_gateway_and_progress(self):
        config = {"channel": "development", "updateRoot": "/tmp/update",
                  "statusFile": "/tmp/status", "serverTokenFile": "/tmp/token",
                  "lifecycleDir": "/tmp/lifecycle", "project": "fixture-project",
                  "region": "asia-northeast3", "gatewayService": "fixture-gateway",
                  "dataDir": "/tmp/data", "serverUnit": "server.service",
                  "proxyUnit": "proxy.service", "publicKey": "/tmp/key"}
        with patch.object(worker, "ServerAdmission") as admission, patch.object(worker, "CloudRunGateway") as gateway, \
             patch.object(worker, "SystemdHost") as host, patch.object(worker, "Updater") as updater, \
             patch.object(worker, "GitHubReleases") as source, patch.object(worker, "ReleasePoller") as poller:
            worker.make_poller(config)
            admission.assert_called_once_with("/tmp/token", "/tmp/lifecycle")
            gateway.assert_called_once_with("fixture-project", "asia-northeast3", "fixture-gateway")
            self.assertEqual(host.call_args.kwargs["proxy_unit"], "proxy.service")
            self.assertEqual(updater.call_args.kwargs["channel"], "development")
            source.assert_called_once_with("development")
            poller.assert_called_once()
