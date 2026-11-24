import http.client
import http.server
import os
import threading
import time
import unittest
from unittest.mock import patch

import momso_anywhere_gateway as gateway


class Upstream(http.server.BaseHTTPRequestHandler):
    seen = []

    def log_message(self, *_args): pass

    def do_GET(self):
        self.seen.append((self.path, dict(self.headers)))
        self.send_response(200)
        self.end_headers()
        if self.path == "/events":
            self.wfile.write(b"data: first\n\n")
            self.wfile.flush()
            time.sleep(0.15)
            self.wfile.write(b"data: second\n\n")
        else:
            self.wfile.write(b"<html>fixture</html>")


class Tests(unittest.TestCase):
    def setUp(self):
        self.auth = patch.object(gateway.Handler, "authorized", return_value=True)
        self.auth.start()
        self.addCleanup(self.auth.stop)
        self.host = patch.dict(os.environ, {"MOMSO_PUBLIC_HOST": "fixture.run.app"})
        self.host.start()
        self.addCleanup(self.host.stop)
        self.upstream = http.server.ThreadingHTTPServer(("127.0.0.1", 0), Upstream)
        self.gateway = http.server.ThreadingHTTPServer(("127.0.0.1", 0), gateway.Handler)
        for server in (self.upstream, self.gateway):
            thread = threading.Thread(target=server.serve_forever, daemon=True)
            thread.start()
            self.addCleanup(thread.join, 2)
            self.addCleanup(server.server_close)
            self.addCleanup(server.shutdown)
        Upstream.seen.clear()

    def get(self, path, headers):
        client = http.client.HTTPConnection("127.0.0.1", self.gateway.server_address[1])
        client.request("GET", path, headers=headers)
        return client, client.getresponse()

    def test_public_ready_does_not_wake_or_expose_server(self):
        self.auth.stop()
        with patch.object(gateway, 'upstream_connection') as upstream, patch.object(gateway, 'start_instance') as wake:
            client, response = self.get('/__momso/ready', {'Host': 'fixture.run.app', 'Origin': 'https://start.example.test'})
            self.assertEqual(response.status, 200)
            self.assertEqual(response.getheader('Access-Control-Allow-Origin'), '*')
            self.assertEqual(response.getheader('Cache-Control'), 'no-store')
            self.assertEqual(response.read(), b'{"ready":true}')
            client.close()
            upstream.assert_not_called(); wake.assert_not_called()

    def test_html_and_stream_are_forwarded_without_leaking_token(self):
        def connect(): return http.client.HTTPConnection("127.0.0.1", self.upstream.server_address[1])
        with patch.dict(os.environ, {"MOMSO_PRIVATE_PROXY_TOKEN": "fixture-secret"}), patch.object(gateway, "upstream_connection", side_effect=connect):
            client, response = self.get("/", {"Host": "fixture.run.app", "Authorization": "Bearer forged", "Cookie": "private=fixture"})
            self.assertIn(b"fixture", response.read())
            client.close()
            client, response = self.get("/events", {"Host": "fixture.run.app"})
            self.assertEqual(response.readline(), b"data: first\n")
            self.assertEqual(response.readline(), b"\n")
            self.assertEqual(response.readline(), b"data: second\n")
            response.read()
            client.close()
        self.assertEqual(Upstream.seen[0][1]["Authorization"], "Bearer fixture-secret")
        self.assertNotIn("Cookie", Upstream.seen[0][1])

    def test_navigation_wakes_unavailable_vm_and_retries(self):
        calls = []
        def connect():
            calls.append(1)
            if len(calls) == 1:
                raise OSError("fixture stopped")
            return http.client.HTTPConnection("127.0.0.1", self.upstream.server_address[1])
        headers = {"Host": "fixture.run.app", "Sec-Fetch-Mode": "navigate", "Sec-Fetch-Dest": "document"}
        with patch.dict(os.environ, {"MOMSO_PRIVATE_PROXY_TOKEN": "fixture-secret"}), patch.object(gateway, "upstream_connection", side_effect=connect), patch.object(gateway, "start_instance") as wake:
            client, response = self.get("/", headers)
            self.assertEqual(response.status, 200)
            self.assertIn('몸소를 켜고 있습니다.'.encode(), response.read())
            client.close()
            client, response = self.get("/", headers)
            self.assertEqual(response.status, 200)
            self.assertIn(b"fixture", response.read())
            client.close()
        wake.assert_called_once()
        self.assertEqual(len(calls), 2)

    def test_cross_origin_write_is_rejected_before_upstream(self):
        with patch.object(gateway, "upstream_connection") as connect:
            client = http.client.HTTPConnection("127.0.0.1", self.gateway.server_address[1])
            client.request("POST", "/chats", b"{}", {"Host": "fixture.run.app", "Origin": "https://untrusted.example"})
            response = client.getresponse()
            self.assertEqual(response.status, 403)
            response.read()
            client.close()
            connect.assert_not_called()

    def test_background_request_does_not_wake_vm(self):
        with patch.dict(os.environ, {"MOMSO_PRIVATE_PROXY_TOKEN": "fixture-secret"}), patch.object(gateway, "upstream_connection", side_effect=OSError("offline")), patch.object(gateway, "start_instance") as wake:
            client, response = self.get("/events", {"Host": "fixture.run.app"})
            self.assertEqual(response.status, 503)
            response.read()
            client.close()
            wake.assert_not_called()

    def test_bad_certificate_is_not_treated_as_sleeping_vm(self):
        headers = {"Host": "fixture.run.app", "Sec-Fetch-Mode": "navigate", "Sec-Fetch-Dest": "document"}
        with patch.dict(os.environ, {"MOMSO_PRIVATE_PROXY_TOKEN": "fixture-secret"}), patch.object(gateway, "upstream_connection", side_effect=gateway.ssl.SSLError("wrong certificate")), patch.object(gateway, "start_instance") as wake:
            client, response = self.get("/", headers)
            self.assertEqual(response.status, 503)
            response.read()
            client.close()
            wake.assert_not_called()


if __name__ == "__main__":
    unittest.main()
