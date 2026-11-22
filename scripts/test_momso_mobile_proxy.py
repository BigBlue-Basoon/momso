import http.client
import http.server
import os
import threading
import time
import unittest
from unittest.mock import patch

import momso_mobile_proxy as proxy


class Upstream(http.server.BaseHTTPRequestHandler):
    observed = []

    def log_message(self, *_args): pass

    def do_POST(self):
        self.observed.append(dict(self.headers))
        self.rfile.read(int(self.headers.get("Content-Length", "0")))
        body = b'{"state":"ready"}'
        self.send_response(200)
        self.send_header("Content-Length", str(len(body)))
        self.end_headers()
        self.wfile.write(body)

    def do_GET(self):
        self.observed.append(dict(self.headers))
        if self.path in ("/", "/assets/app.js", "/api/chats"):
            body = {"/": b'<html><script src="/assets/app.js"></script></html>', "/assets/app.js": b'fetch("/api/chats")', "/api/chats": b'{"chats":[]}'}[self.path]
            self.send_response(200)
            self.send_header("Content-Length", str(len(body)))
            self.end_headers()
            self.wfile.write(body)
            return
        self.send_response(200)
        self.send_header("Content-Type", "text/event-stream")
        self.end_headers()
        self.wfile.write(b'event: chat-delta\ndata: {"delta":"fixture"}\n\n')
        self.wfile.flush()
        time.sleep(0.2)
        self.wfile.write(b'event: chat-done\ndata: {"state":"completed"}\n\n')


class ProxyTests(unittest.TestCase):
    def test_same_origin_only_and_server_token_stays_internal(self):
        upstream = http.server.ThreadingHTTPServer(("127.0.0.1", 0), Upstream)
        bridge = http.server.ThreadingHTTPServer(("127.0.0.1", 0), proxy.Handler)
        self.addCleanup(upstream.server_close)
        self.addCleanup(bridge.server_close)
        for server in (upstream, bridge):
            thread = threading.Thread(target=server.serve_forever, daemon=True)
            thread.start()
            self.addCleanup(thread.join, 2)
            self.addCleanup(server.shutdown)
        Upstream.observed.clear()
        real_http = http.client.HTTPConnection
        upstream_port = upstream.server_address[1]
        bridge_port = bridge.server_address[1]
        def to_fixture(host, port, **kwargs):
            self.assertEqual((host, port), ("127.0.0.1", 8765))
            self.assertNotIn("timeout", kwargs)
            return real_http("127.0.0.1", upstream_port, **kwargs)
        with patch.dict(os.environ, {"MOMSO_MOBILE_HOST": "mobile.example.test"}), patch.object(proxy, "read_token", return_value="fixture-secret"), patch.object(proxy.http.client, "HTTPConnection", side_effect=to_fixture):
            for path, expected in (("/", b"<html>"), ("/assets/app.js", b"fetch"), ("/api/chats", b'"chats"')):
                client = real_http("127.0.0.1", bridge_port)
                client.request("GET", path, headers={"Host": "mobile.example.test"})
                response = client.getresponse()
                self.assertEqual(response.status, 200)
                self.assertIn(expected, response.read())
                client.close()
            Upstream.observed.clear()
            client = real_http("127.0.0.1", bridge_port)
            client.request("POST", "/chats", b"{}", {"Host": "mobile.example.test", "Origin": "https://other.example.test"})
            response = client.getresponse()
            self.assertEqual(response.status, 403)
            response.read()
            client.close()
            self.assertFalse(Upstream.observed)
            client = real_http("127.0.0.1", bridge_port)
            client.request("POST", "/chats", b"{}", {"Host": "mobile.example.test", "Origin": "https://mobile.example.test", "Authorization": "Bearer attacker", "Cookie": "fixture-cookie", "Cf-Access-Jwt-Assertion": "fixture-assertion"})
            response = client.getresponse()
            self.assertEqual(response.status, 200)
            self.assertEqual(response.getheader("Cache-Control"), "no-store")
            self.assertNotIn(b"fixture-secret", response.read())
            client.close()
            client = real_http("127.0.0.1", bridge_port)
            client.request("GET", "/turns/fixture/events", headers={"Host": "mobile.example.test"})
            response = client.getresponse()
            self.assertEqual(response.status, 200)
            self.assertEqual(response.readline(), b"event: chat-delta\n")
            self.assertIn(b"fixture", response.readline())
            self.assertEqual(response.readline(), b"\n")
            self.assertEqual(response.readline(), b"event: chat-done\n")
            self.assertIn(b"completed", response.read())
            client.close()
        self.assertEqual(Upstream.observed[0]["Authorization"], "Bearer fixture-secret")
        self.assertNotIn("Cookie", Upstream.observed[0])
        self.assertNotIn("Cf-Access-Jwt-Assertion", Upstream.observed[0])


if __name__ == "__main__":
    unittest.main()
