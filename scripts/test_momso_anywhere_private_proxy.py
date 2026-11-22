import hashlib
import http.client
import http.server
import os
import tempfile
import threading
import unittest
from pathlib import Path
from unittest.mock import MagicMock, patch

import momso_anywhere_gateway as gateway
import momso_anywhere_vm_proxy as private
import momso_mobile_proxy


class Upstream(http.server.BaseHTTPRequestHandler):
    seen = []
    def log_message(self, *_args): pass
    def do_GET(self):
        self.seen.append(self.headers.get("Authorization"))
        self.send_response(200)
        self.end_headers()
        self.wfile.write(b"fixture")


class Tests(unittest.TestCase):
    def test_certificate_fingerprint_is_checked_before_requests(self):
        sock = MagicMock()
        sock.getpeercert.return_value = b"fixture-certificate"
        conn = gateway.PinnedHTTPSConnection("fixture.example")
        def fake_connect(self): self.sock = sock
        with patch.object(http.client.HTTPSConnection, "connect", fake_connect):
            with patch.dict(os.environ, {"MOMSO_PRIVATE_CERT_SHA256": hashlib.sha256(b"fixture-certificate").hexdigest()}):
                conn.connect()
            with patch.dict(os.environ, {"MOMSO_PRIVATE_CERT_SHA256": "0" * 64}):
                with self.assertRaises(gateway.ssl.SSLError): conn.connect()
        sock.close.assert_called()

    def test_proxy_token_is_required_and_replaced_with_server_token(self):
        with tempfile.TemporaryDirectory() as directory:
            tokenfile = Path(directory) / "proxy-token"
            tokenfile.write_text("fixture-proxy-token", encoding="utf-8")
            upstream = http.server.ThreadingHTTPServer(("127.0.0.1", 0), Upstream)
            proxy = http.server.ThreadingHTTPServer(("127.0.0.1", 0), private.Handler)
            threads = [threading.Thread(target=x.serve_forever, daemon=True) for x in (upstream, proxy)]
            for thread in threads: thread.start()
            real_http = http.client.HTTPConnection
            try:
                def connect(host, port, **kwargs):
                    self.assertEqual((host, port), ("127.0.0.1", 8765))
                    return real_http("127.0.0.1", upstream.server_address[1], **kwargs)
                with patch.dict(os.environ, {"MOMSO_PRIVATE_PROXY_TOKEN_FILE": str(tokenfile), "MOMSO_MOBILE_HOST": "vm.internal"}), patch.object(momso_mobile_proxy, "read_token", return_value="fixture-server-token"), patch.object(momso_mobile_proxy.http.client, "HTTPConnection", side_effect=connect):
                    for token, status in (("wrong", 403), ("fixture-proxy-token", 200)):
                        client = real_http("127.0.0.1", proxy.server_address[1])
                        client.request("GET", "/", headers={"Authorization": "Bearer " + token})
                        response = client.getresponse()
                        self.assertEqual(response.status, status)
                        response.read()
                        client.close()
                self.assertEqual(Upstream.seen[-1:], ["Bearer fixture-server-token"])
            finally:
                for server in (upstream, proxy): server.shutdown(); server.server_close()
                for thread in threads: thread.join(2)


if __name__ == "__main__": unittest.main()
