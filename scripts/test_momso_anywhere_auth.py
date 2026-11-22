import hashlib
import http.client
import http.server
import json
import os
import tempfile
import threading
import unittest
import urllib.parse
from pathlib import Path
from unittest.mock import patch

import momso_anywhere_auth as auth
import momso_anywhere_gateway as gateway


class Tests(unittest.TestCase):
    def setUp(self):
        self.tmp = tempfile.TemporaryDirectory()
        self.addCleanup(self.tmp.cleanup)
        key = Path(self.tmp.name) / "key"
        key.write_bytes(b"x" * 64)
        self.env = patch.dict(os.environ, {"MOMSO_SESSION_KEY_FILE": str(key), "MOMSO_GOOGLE_CLIENT_ID": "fixture-client", "MOMSO_OWNER_SUB": "fixture-owner"})
        self.env.start()
        self.addCleanup(self.env.stop)

    def test_only_owner_with_matching_nonce_gets_session(self):
        state = auth.challenge("https://fixture.run.app")
        nonce = auth._b64(hashlib.sha256(state.encode()).digest())
        verifier = lambda token, client: {"sub": "fixture-owner", "nonce": nonce}
        session = auth.finish(state, state, "fixture-id-token", "https://fixture.run.app", verifier)
        self.assertTrue(auth.valid_session(session))
        self.assertFalse(auth.valid_session(session + "x"))
        with self.assertRaises(ValueError):
            auth.finish(state, state, "fixture", "https://other.run.app", verifier)
        with self.assertRaises(ValueError):
            auth.finish(state, "other", "fixture", "https://fixture.run.app", verifier)
        with self.assertRaises(ValueError):
            auth.finish(state, state, "fixture", "https://fixture.run.app", lambda *_: {"sub": "other", "nonce": nonce})
        with self.assertRaises(ValueError):
            auth.finish(state, state, "fixture", "https://fixture.run.app", lambda *_: {"sub": "fixture-owner", "nonce": "other"})

    def test_expired_or_type_confused_state_is_rejected(self):
        with patch.object(auth.time, "time", return_value=1000):
            state = auth.challenge("https://fixture.run.app")
        with patch.object(auth.time, "time", return_value=1301):
            with self.assertRaises(ValueError):
                auth.finish(state, state, "fixture", "https://fixture.run.app", lambda *_: {})
        self.assertFalse(auth.valid_session(state))

    def test_session_expires_after_thirty_days(self):
        with patch.object(auth.time, "time", return_value=1000):
            session = auth._sign("session", {"iat": 1000, "sub": "fixture-owner"})
        with patch.object(auth.time, "time", return_value=1000 + auth.SESSION_SECONDS):
            self.assertTrue(auth.valid_session(session))
        with patch.object(auth.time, "time", return_value=1001 + auth.SESSION_SECONDS):
            self.assertFalse(auth.valid_session(session))

    def test_manifest_and_icons_are_public_but_other_files_are_not(self):
        public = Path(self.tmp.name) / "public"
        (public / "app-icons").mkdir(parents=True)
        (public / "manifest.webmanifest").write_text('{"name":"momso"}', encoding="utf-8")
        (public / "app-icons/icon-192.png").write_bytes(b"fixture")
        (public / "private.json").write_text("private", encoding="utf-8")
        with patch.dict(os.environ, {"MOMSO_PUBLIC_HOST": "fixture.run.app", "MOMSO_PUBLIC_ASSETS_DIR": str(public), "MOMSO_LOGIN_ORIGIN": "https://login.example.test"}):
            server = http.server.ThreadingHTTPServer(("127.0.0.1", 0), gateway.Handler)
            thread = threading.Thread(target=server.serve_forever, daemon=True)
            thread.start()
            try:
                for path, status in (("/manifest.webmanifest", 200), ("/app-icons/icon-192.png", 200), ("/assets/app.js", 401), ("/private.json", 401), ("/chats", 401), ("/app-icons/../private.json", 401)):
                    client = http.client.HTTPConnection("127.0.0.1", server.server_address[1])
                    client.request("GET", path, headers={"Host": "fixture.run.app"})
                    response = client.getresponse()
                    self.assertEqual(response.status, status, path)
                    response.read()
                    client.close()
                client = http.client.HTTPConnection("127.0.0.1", server.server_address[1])
                client.request("GET", "/manifest.webmanifest", headers={"Host": "fixture.run.app", "Cookie": "stale=fixture"})
                response = client.getresponse()
                self.assertEqual(response.status, 200)
                self.assertEqual(response.getheader("Content-Type"), "application/manifest+json")
                response.read(); client.close()
                client = http.client.HTTPConnection("127.0.0.1", server.server_address[1])
                client.request("GET", "/", headers={"Host": "fixture.run.app", "Sec-Fetch-Mode": "navigate", "Sec-Fetch-Dest": "document"})
                response = client.getresponse()
                self.assertEqual(response.status, 200)
                self.assertEqual(response.getheader("Referrer-Policy"), "strict-origin")
                self.assertIn("SameSite=Lax", response.getheader("Set-Cookie"))
                body = response.read()
                self.assertIn(b"location.replace(", body)
                self.assertIn(b'http-equiv="refresh"', body)
                client.close()
            finally:
                server.shutdown(); server.server_close(); thread.join(2)

    def test_gateway_requires_login_and_sets_secure_owner_session(self):
        with patch.dict(os.environ, {"MOMSO_PUBLIC_HOST": "fixture.run.app"}):
            server = http.server.ThreadingHTTPServer(("127.0.0.1", 0), gateway.Handler)
            thread = threading.Thread(target=server.serve_forever, daemon=True)
            thread.start()
            try:
                def request(method, path, body=None, headers=None):
                    client = http.client.HTTPConnection("127.0.0.1", server.server_address[1])
                    client.request(method, path, body, {"Host": "fixture.run.app", **(headers or {})})
                    response = client.getresponse()
                    data = response.read()
                    return client, response, data
                client, response, _ = request("GET", "/chats")
                self.assertEqual(response.status, 401)
                client.close()
                client, response, data = request("GET", "/__momso/auth/challenge")
                state = json.loads(data)["state"]
                challenge_cookie = response.getheader("Set-Cookie").split(";", 1)[0]
                client.close()
                nonce = auth._b64(hashlib.sha256(state.encode()).digest())
                fake = lambda *_: {"sub": "fixture-owner", "nonce": nonce}
                # Defaults bind at definition time; patch finish itself to inject the verifier.
                real_finish = auth.finish
                with patch.object(auth, "finish", side_effect=lambda *args: real_finish(*args, verifier=fake)):
                    body = json.dumps({"state": state, "credential": "fixture"})
                    client, response, _ = request("POST", "/__momso/auth/finish", body, {"Origin": "https://fixture.run.app", "Cookie": challenge_cookie})
                    self.assertEqual(response.status, 200)
                    self.assertIn("Secure; HttpOnly; SameSite=Strict", response.getheader("Set-Cookie"))
                    session_cookie = response.getheader("Set-Cookie").split(";", 1)[0]
                    client.close()
                client, response, _ = request("POST", "/__momso/auth/logout", "", {"Origin": "https://fixture.run.app", "Cookie": session_cookie})
                self.assertEqual(response.status, 200)
                self.assertIn("Max-Age=0", response.getheader("Set-Cookie"))
                client.close()
            finally:
                server.shutdown(); server.server_close(); thread.join(2)

    def test_navigation_redirect_and_fragment_return_keep_token_out_of_response(self):
        with patch.dict(os.environ, {"MOMSO_PUBLIC_HOST": "fixture.run.app", "MOMSO_LOGIN_ORIGIN": "https://login.example.test"}):
            server = http.server.ThreadingHTTPServer(("127.0.0.1", 0), gateway.Handler)
            thread = threading.Thread(target=server.serve_forever, daemon=True)
            thread.start()
            try:
                client = http.client.HTTPConnection("127.0.0.1", server.server_address[1])
                client.request("GET", "/", headers={"Host": "fixture.run.app", "Sec-Fetch-Mode": "navigate", "Sec-Fetch-Dest": "document"})
                response = client.getresponse()
                body = response.read().decode()
                location = json.loads(body.split("<script>location.replace(", 1)[1].split(");</script>", 1)[0])
                state = urllib.parse.parse_qs(urllib.parse.urlsplit(location).query)["state"][0]
                cookie = response.getheader("Set-Cookie").split(";", 1)[0]
                self.assertEqual(response.status, 200)
                client.close()
                client = http.client.HTTPConnection("127.0.0.1", server.server_address[1])
                client.request("GET", "/__momso/auth/return", headers={"Host": "fixture.run.app", "Cookie": cookie})
                response = client.getresponse()
                body = response.read()
                self.assertEqual(response.status, 200)
                self.assertEqual(response.getheader("Cache-Control"), "no-store")
                self.assertIn(b'history.replaceState', body)
                self.assertNotIn(state.encode(), body)
                client.close()
                nonce = auth._b64(hashlib.sha256(state.encode()).digest())
                original = auth.finish
                with patch.object(auth, "finish", side_effect=lambda *args: original(*args, verifier=lambda *_: {"sub": "fixture-owner", "nonce": nonce})):
                    client = http.client.HTTPConnection("127.0.0.1", server.server_address[1])
                    client.request("POST", "/__momso/auth/finish", json.dumps({"state": state, "credential": "fixture-token"}), {"Host": "fixture.run.app", "Origin": "https://fixture.run.app", "Cookie": cookie})
                    response = client.getresponse()
                    self.assertEqual(response.status, 200)
                    self.assertIn("SameSite=Strict", response.getheader("Set-Cookie"))
                    self.assertNotIn(b"fixture-token", response.read())
                    client.close()
            finally:
                server.shutdown(); server.server_close(); thread.join(2)


if __name__ == "__main__": unittest.main()
