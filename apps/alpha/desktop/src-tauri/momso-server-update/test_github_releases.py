import http.server
import json
from pathlib import Path
import subprocess
import threading
import unittest
from unittest.mock import Mock
from urllib.parse import parse_qs, urlparse

from github_releases import GitHubReleases, RateLimited, ReleasePoller
import install_worker
from test_update import UpdateTests
from update import UpdateError, _extract


class ReleaseDiscoveryTests(UpdateTests):
    def setUp(self):
        super().setUp()
        self.items = []
        self.status = 200
        self.pages = None
        fixture = self

        class Handler(http.server.BaseHTTPRequestHandler):
            def do_GET(self):
                self.send_response(fixture.status)
                self.send_header("Content-Type", "application/json")
                self.end_headers()
                page = int(parse_qs(urlparse(self.path).query).get("page", [1])[0])
                items = fixture.pages.get(page, []) if fixture.pages is not None else fixture.items
                self.wfile.write(json.dumps(items).encode())

            def log_message(self, *_args):
                pass

        self.api = http.server.ThreadingHTTPServer(("127.0.0.1", 0), Handler)
        self.api_thread = threading.Thread(target=self.api.serve_forever, daemon=True)
        self.api_thread.start()
        self.api_url = "http://127.0.0.1:%d/releases" % self.api.server_port

    def tearDown(self):
        self.api.shutdown()
        self.api.server_close()
        self.api_thread.join()
        super().tearDown()

    def release(self, published, prerelease, *, draft=False, signature=True):
        assets = [{"name": "release.json", "state": "uploaded", "browser_download_url": self.base + "/manifest.json"}]
        if signature:
            assets.append({"name": "release.sig", "state": "uploaded", "browser_download_url": self.base + "/manifest.sig"})
        return {"published_at": published, "prerelease": prerelease, "draft": draft, "assets": assets}

    def source(self, channel="production"):
        return GitHubReleases(channel, api_url=self.api_url, allow_http_for_tests=True)

    def test_channel_and_latest_release(self):
        self.items = [self.release("2026-01-02T00:00:00Z", False),
                      self.release("2026-01-04T00:00:00Z", True),
                      self.release("2026-01-05T00:00:00Z", False, draft=True),
                      self.release("2026-01-06T00:00:00Z", False, signature=False)]
        self.items[0]["assets"][0]["browser_download_url"] = self.base + "/production.json"
        self.items[1]["assets"][0]["browser_download_url"] = self.base + "/development.json"
        self.assertEqual(self.source().discover()[0], self.base + "/production.json")
        self.assertEqual(self.source("development").discover()[0], self.base + "/development.json")
        self.items = [self.release("2026-01-04T00:00:00Z", True)]
        self.assertIsNone(self.source().discover())

    def test_fake_github_signed_update_and_sequence(self):
        self.items = [self.release("2026-01-02T00:00:00Z", False)]
        poller = ReleasePoller(self.source(), self.updater)
        self.assertEqual(poller.poll(), "completed")
        self.assertEqual(self.host.sequence, 2)
        self.assertEqual(poller.poll(), "up_to_date")

    def test_pagination_checks_later_releases(self):
        self.pages = {1: [self.release("2026-01-01T00:00:00Z", True)] * 100,
                      2: [self.release("2026-01-02T00:00:00Z", False)]}
        self.assertIsNotNone(self.source().discover())

    def test_periodic_worker_retries_after_failure(self):
        updater = Mock(channel="production")
        updater.recover.return_value = "no_recovery"
        updater.install.side_effect = [UpdateError("update_unavailable"), "completed"]
        source = Mock(channel="production")
        source.discover.return_value = ("manifest", "signature")
        stop = Mock()
        stop.is_set.return_value = False
        stop.wait.side_effect = [False, True]
        ReleasePoller(source, updater).run_forever(stop, interval_seconds=3600)
        self.assertEqual(updater.install.call_count, 2)
        self.assertEqual(updater.recover.call_count, 2)

    def test_rate_limit_delays_next_attempt(self):
        updater = Mock(channel="production")
        source = Mock(channel="production")
        source.discover.side_effect = RateLimited(7200)
        stop = Mock()
        stop.is_set.return_value = False
        stop.wait.return_value = True
        ReleasePoller(source, updater).run_forever(stop, interval_seconds=3600)
        stop.wait.assert_called_once_with(7200)

    def test_failure_retried_next_tick_and_busy_waits(self):
        self.items = [self.release("2026-01-02T00:00:00Z", False)]
        poller = ReleasePoller(self.source(), self.updater)
        self.status = 503
        with self.assertRaisesRegex(UpdateError, "release_discovery_unavailable"):
            poller.poll()
        self.status = 200
        self.host.available = False
        self.assertEqual(poller.poll(), "busy")
        self.host.available = True
        self.assertEqual(poller.poll(), "completed")

    def test_install_then_discover_and_update_from_fake_github(self):
        flat = self.root / "flat"
        flat.mkdir()
        _extract((self.server_dir / "bundle.tar.gz").read_bytes(), flat)
        units, worker = self.root / "units", self.root / "worker"
        units.mkdir(); worker.mkdir()
        (units / "momso-private-proxy.service").write_text(
            "[Unit]\nRequires=momso-server.service\n[Service]\nExecStart=/usr/bin/python3 "
            + str(flat) + "/momso_anywhere_vm_proxy.py\n")
        config = {"updateRoot": str(self.updater.root), "channel": "production"}
        calls = Mock()
        install_worker.install(flat, units, worker, self.root / "update-config.json", config,
                               self.updater, self.base + "/manifest.json", self.base + "/manifest.sig", run=calls)
        self.host.release = str((self.updater.root / "current").resolve())
        self.host.sequence = 2
        manifest_path = self.server_dir / "manifest.json"
        manifest = json.loads(manifest_path.read_text())
        manifest["sequence"] = 3
        manifest_path.write_text(json.dumps(manifest))
        subprocess.run(["openssl", "pkeyutl", "-sign", "-inkey", str(self.private), "-rawin", "-in", str(manifest_path), "-out", str(self.server_dir / "manifest.sig")], check=True, capture_output=True)
        self.items = [self.release("2026-01-02T00:00:00Z", False)]
        self.assertEqual(ReleasePoller(self.source(), self.updater).poll(), "completed")
        self.assertEqual(self.host.sequence, 3)
        self.assertEqual(Path(self.host.release).stat().st_mode & 0o777, 0o755)
        self.assertEqual((flat / "momso-server").read_text(), "momso-server")
        self.assertTrue(any(event["state"] == "completed" for event in self.host.events))
