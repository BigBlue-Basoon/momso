import http.client
import http.server
import hashlib
import hmac
import os
import threading
import time
import unittest
from unittest.mock import patch

import momso_anywhere_gateway as gateway


class Upstream(http.server.BaseHTTPRequestHandler):
    seen = []

    def log_message(self, *_args): pass

    def do_POST(self):
        data = self.rfile.read(int(self.headers.get('Content-Length', '0')))
        self.seen.append((self.path, dict(self.headers), len(data)))
        self.send_response(200)
        self.end_headers()
        self.wfile.write(str(len(data)).encode())

    def do_GET(self):
        self.seen.append((self.path, dict(self.headers)))
        if self.path == '/upstream-forbidden':
            self.send_response(403)
            self.send_header('X-Momso-Response', 'gateway')
            self.end_headers()
            self.wfile.write(b'upstream forbidden')
            return
        if self.path == '/health':
            self.send_response(200)
            self.send_header('Content-Type', 'application/json')
            self.end_headers()
            self.wfile.write(b'{"state":"ready"}')
            return
        self.send_response(200)
        self.end_headers()
        if self.path == "/events":
            self.wfile.write(b"data: first\n\n")
            self.wfile.flush()
            time.sleep(0.15)
            self.wfile.write(b"data: second\n\n")
        else:
            self.wfile.write(b"<html>fixture</html>")


class FakeEdge(http.server.BaseHTTPRequestHandler):
    def log_message(self, *_args): pass
    def do_GET(self):
        self.send_response(403)
        self.send_header('Content-Type', 'text/html')
        self.end_headers()
        self.wfile.write(b'<html>forbidden</html>')


class Tests(unittest.TestCase):
    def setUp(self):
        limiter = patch.object(gateway, '_wake_until', 0.0)
        limiter.start()
        self.addCleanup(limiter.stop)
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

    def test_private_health_probe_requires_proof_and_strips_it_upstream(self):
        self.auth.stop()
        with patch.dict(os.environ, {'MOMSO_PRIVATE_PROXY_TOKEN': 'fixture-secret'}), patch.object(gateway, 'upstream_connection', side_effect=lambda: http.client.HTTPConnection('127.0.0.1', self.upstream.server_address[1])):
            client, response = self.get('/__momso/proxy-health', {'Host': 'fixture.run.app'})
            self.assertEqual(response.status, 403); response.read(); client.close()
            proof = hmac.new(b'fixture-secret', b'momso-proxy-health-v1', hashlib.sha256).hexdigest()
            client, response = self.get('/__momso/proxy-health', {'Host': 'fixture.run.app', 'X-Momso-Health-Proof': proof})
            self.assertEqual(response.status, 200); self.assertEqual(response.read(), b'{"state":"ready"}'); client.close()
            self.assertEqual(Upstream.seen[-1][0], '/health')
            self.assertNotIn('X-Momso-Health-Proof', Upstream.seen[-1][1])

    def test_private_health_probe_cors_only_allows_login_origin(self):
        self.auth.stop()
        with patch.dict(os.environ, {'MOMSO_PRIVATE_PROXY_TOKEN': 'fixture-secret', 'MOMSO_LOGIN_ORIGIN': 'https://login.example.test'}):
            for origin, expected in [('https://login.example.test', 204), ('https://other.example.test', 403)]:
                client = http.client.HTTPConnection('127.0.0.1', self.gateway.server_address[1])
                client.request('OPTIONS', '/__momso/proxy-health', headers={'Host': 'fixture.run.app', 'Origin': origin, 'Access-Control-Request-Method': 'GET', 'Access-Control-Request-Headers': 'x-momso-health-proof'})
                response = client.getresponse()
                self.assertEqual(response.status, expected)
                if expected == 204: self.assertEqual(response.getheader('Access-Control-Allow-Origin'), origin)
                response.read(); client.close()

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

    def test_repeated_anonymous_requests_do_not_contact_or_wake_vm(self):
        self.auth.stop()
        with patch.dict(os.environ, {'MOMSO_PRIVATE_PROXY_TOKEN': 'fixture-secret'}), \
             patch.object(gateway, 'upstream_connection') as upstream, \
             patch.object(gateway, 'start_instance') as wake:
            for _ in range(50):
                for path, status in (('/__momso/ready', 200), ('/health', 401),
                                     ('/chats', 401), ('/__momso/proxy-health', 403)):
                    client, response = self.get(path, {'Host': 'fixture.run.app'})
                    self.assertEqual(response.status, status)
                    response.read(); client.close()
            upstream.assert_not_called(); wake.assert_not_called()

    def test_invalid_host_is_json_with_gateway_marker(self):
        client, response = self.get('/__momso/ready', {'Host': 'other.example'})
        self.assertEqual(response.status, 403)
        self.assertEqual(response.getheader('X-Momso-Response'), 'gateway')
        self.assertEqual(response.getheader('Content-Type'), 'application/json')
        self.assertIn(b'invalid_host', response.read())
        client.close()

    def test_invalid_content_length_is_a_json_rejection(self):
        with patch.dict(os.environ, {'MOMSO_PRIVATE_PROXY_TOKEN': 'fixture-secret'}):
            client, response = self.get('/chats', {'Host': 'fixture.run.app', 'Content-Length': '-1'})
            self.assertEqual(response.status, 400)
            self.assertEqual(response.getheader('X-Momso-Response'), 'gateway')
            self.assertIn(b'invalid_request', response.read())
            client.close()

    def test_fake_edge_rejection_has_no_gateway_marker(self):
        edge = http.server.ThreadingHTTPServer(('127.0.0.1', 0), FakeEdge)
        thread = threading.Thread(target=edge.serve_forever, daemon=True); thread.start()
        try:
            client = http.client.HTTPConnection('127.0.0.1', edge.server_address[1])
            client.request('GET', '/')
            response = client.getresponse()
            self.assertEqual(response.status, 403)
            self.assertIsNone(response.getheader('X-Momso-Response'))
            response.read(); client.close()
        finally:
            edge.shutdown(); edge.server_close(); thread.join(2)

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

    def test_owner_upload_passes_through(self):
        def connect(): return http.client.HTTPConnection('127.0.0.1', self.upstream.server_address[1])
        with patch.dict(os.environ, {'MOMSO_PRIVATE_PROXY_TOKEN': 'fixture-secret'}), patch.object(gateway, 'upstream_connection', side_effect=connect):
            client = http.client.HTTPConnection('127.0.0.1', self.gateway.server_address[1])
            client.request('POST', '/uploads', body=b'x' * (1024 * 1024), headers={'Host': 'fixture.run.app', 'Origin': 'https://fixture.run.app'})
            response = client.getresponse()
            self.assertEqual((response.status, response.read()), (200, b'1048576'))
            self.assertEqual(Upstream.seen[-1][2], 1024 * 1024)
            client.close()

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
            self.assertEqual(response.getheader('X-Momso-Response'), 'gateway')
            self.assertEqual(response.getheader('Content-Type'), 'application/json')
            self.assertIn(b'origin_not_allowed', response.read())
            client.close()
            connect.assert_not_called()

    def test_forwarded_rejection_is_marked_upstream_not_gateway(self):
        def connect(): return http.client.HTTPConnection('127.0.0.1', self.upstream.server_address[1])
        with patch.dict(os.environ, {'MOMSO_PRIVATE_PROXY_TOKEN': 'fixture-secret'}), patch.object(gateway, 'upstream_connection', side_effect=connect):
            client, response = self.get('/upstream-forbidden', {'Host': 'fixture.run.app'})
            self.assertEqual(response.status, 403)
            self.assertEqual(response.getheader('X-Momso-Response'), 'upstream')
            response.read(); client.close()

    def test_authenticated_fetch_wakes_vm_and_reuses_recent_request(self):
        with patch.dict(os.environ, {"MOMSO_PRIVATE_PROXY_TOKEN": "fixture-secret"}), patch.object(gateway, "upstream_connection", side_effect=OSError("offline")), patch.object(gateway, "start_instance") as wake:
            for _ in range(3):
                client, response = self.get("/events", {"Host": "fixture.run.app"})
                self.assertEqual(response.status, 503)
                self.assertEqual(response.getheader('X-Momso-Response'), 'gateway')
                self.assertEqual(response.getheader('Retry-After'), '5')
                self.assertEqual(response.read(), b'{"error":"waking","retryAfterSeconds":5}')
                client.close()
            wake.assert_called_once()

    def test_unauthenticated_fetch_never_wakes_vm(self):
        self.auth.stop()
        with patch.object(gateway, 'start_instance') as wake, patch.object(gateway, 'upstream_connection') as upstream:
            client, response = self.get('/chats', {'Host': 'fixture.run.app'})
            self.assertEqual(response.status, 401)
            response.read(); client.close()
            wake.assert_not_called(); upstream.assert_not_called()

    def test_valid_owner_cookie_fetch_wakes_vm(self):
        self.auth.stop()
        with patch.dict(os.environ, {'MOMSO_PRIVATE_PROXY_TOKEN': 'fixture-secret'}), patch.object(gateway.auth, 'valid_session', return_value=True), patch.object(gateway, 'upstream_connection', side_effect=OSError('offline')), patch.object(gateway, 'start_instance') as wake:
            client, response = self.get('/chats', {'Host': 'fixture.run.app', 'Cookie': '__Host-momso-session=fixture'})
            self.assertEqual(response.status, 503)
            self.assertIn(b'"error":"waking"', response.read())
            client.close(); wake.assert_called_once()

    def test_wake_does_not_start_an_already_starting_instance(self):
        from momso_wake import main as wake_module
        calls = []
        def api(url, **kwargs):
            calls.append((url, kwargs))
            return {'access_token': 'fixture'} if kwargs.get('metadata') else {'status': 'STARTING'}
        with patch.dict(os.environ, {'MOMSO_PROJECT': 'fixture-project', 'MOMSO_ZONE': 'fixture-zone', 'MOMSO_INSTANCE': 'fixture-instance'}), patch.object(wake_module, 'api_request', side_effect=api):
            self.assertEqual(wake_module.wake(), 'STARTING')
        self.assertFalse(any(call[1].get('method') == 'POST' for call in calls))

    def test_concurrent_owner_requests_share_wake(self):
        barrier = threading.Barrier(6)
        def action():
            barrier.wait()
            gateway.request_wake()
        with patch.object(gateway, 'start_instance', side_effect=lambda: time.sleep(0.05)) as wake:
            threads = [threading.Thread(target=action) for _ in range(6)]
            for thread in threads: thread.start()
            for thread in threads: thread.join(2)
            self.assertTrue(all(not thread.is_alive() for thread in threads))
            wake.assert_called_once()

    def test_failed_wake_is_not_cached(self):
        with patch.object(gateway, 'start_instance', side_effect=[OSError('fixture'), 'STARTING']) as wake:
            with self.assertRaises(OSError): gateway.request_wake()
            gateway.request_wake()
            self.assertEqual(wake.call_count, 2)

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
