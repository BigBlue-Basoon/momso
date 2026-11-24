"""Customer-owned IAP gateway. The upstream must provide verified private TLS."""
import base64
import http.client
import http.server
import hashlib
import html
import hmac
import os
import ssl
import json
import threading
import time
import urllib.parse
from pathlib import Path
from http.cookies import SimpleCookie

import momso_anywhere_auth as auth
from momso_anywhere_public_assets import ASSETS

HOP = {"connection", "transfer-encoding", "keep-alive", "proxy-connection", "upgrade", "te", "trailer"}
PUBLIC_ASSETS = {"/" + name: (name, "image/png" if name.endswith(".png") else "application/manifest+json") for name in ASSETS}
PAUSE_OWNER = {'/__momso/pause/prepare': 'POST', '/__momso/pause/status': 'GET',
               '/__momso/pause/owner-release': 'POST', '/__momso/pause/reissue': 'POST'}
PAUSE_TICKET = {'/__momso/pause/confirm', '/__momso/pause/release'}

RETURN_SCRIPT = b'''(async()=>{const h=location.hash;history.replaceState(null,"",location.pathname);const p=new URLSearchParams(h.slice(1));try{if(p.getAll("state").length!==1||p.getAll("id_token").length!==1)throw Error();const r=await fetch("/__momso/auth/finish",{method:"POST",credentials:"same-origin",headers:{"Content-Type":"application/json"},body:JSON.stringify({state:p.get("state"),credential:p.get("id_token")})});if(!r.ok)throw Error();location.replace("/")}catch{location.replace("/?auth_error=1")}})();'''


def login_origin():
    origin = os.environ.get("MOMSO_LOGIN_ORIGIN", "")
    parsed = urllib.parse.urlsplit(origin)
    if parsed.scheme != "https" or not parsed.hostname or parsed.username or parsed.password or parsed.path or parsed.query or parsed.fragment or parsed.netloc != parsed.hostname:
        raise ValueError("invalid login origin")
    return origin


class PinnedHTTPSConnection(http.client.HTTPSConnection):
    def connect(self):
        super().connect()
        expected = os.environ["MOMSO_PRIVATE_CERT_SHA256"]
        if len(expected) != 64 or not hmac.compare_digest(hashlib.sha256(self.sock.getpeercert(binary_form=True)).hexdigest(), expected.lower()):
            self.close()
            raise ssl.SSLError("private server certificate does not match")


def upstream_connection():
    hostname = os.environ["MOMSO_PRIVATE_HOST"]
    if not hostname or any(c not in "abcdefghijklmnopqrstuvwxyz0123456789.-" for c in hostname):
        raise ValueError("invalid private host")
    port = int(os.environ.get("MOMSO_PRIVATE_PORT", "8765"))
    context = ssl.SSLContext(ssl.PROTOCOL_TLS_CLIENT)
    context.check_hostname = False
    context.verify_mode = ssl.CERT_NONE
    return PinnedHTTPSConnection(hostname, port, timeout=10, context=context)


def start_instance():
    from momso_wake.main import wake
    return wake()


_wake_lock = threading.Lock()
_wake_until = 0.0
WAKE_RETRY_SECONDS = 5


def request_wake():
    global _wake_until
    with _wake_lock:
        if time.monotonic() < _wake_until:
            return
        start_instance()
        _wake_until = time.monotonic() + WAKE_RETRY_SECONDS


class Handler(http.server.BaseHTTPRequestHandler):
    protocol_version = "HTTP/1.0"

    def log_message(self, *_args):
        pass

    def end_headers(self):
        self.send_header("X-Frame-Options", "DENY")
        self.send_header("X-Content-Type-Options", "nosniff")
        self.send_header('X-Momso-Response', getattr(self, '_response_source', 'gateway'))
        if getattr(self, '_pause_cors_origin', None):
            self.send_header('Access-Control-Allow-Origin', self._pause_cors_origin)
            self.send_header('Access-Control-Expose-Headers', 'X-Momso-Response')
            self.send_header('Vary', 'Origin')
        super().end_headers()

    def send_error(self, code, message=None, explain=None):
        errors = {400: 'invalid_request', 403: 'forbidden', 404: 'not_found', 405: 'method_not_allowed',
                  411: 'length_required', 414: 'request_too_long', 431: 'headers_too_large',
                  500: 'internal_error', 501: 'method_not_supported', 503: 'gateway_unavailable', 505: 'http_version_not_supported'}
        self.json_reply(code, {'error': errors.get(code, 'request_rejected')})

    def json_reply(self, status, body, cookies=(), extra_headers=()):
        data = json.dumps(body, separators=(",", ":")).encode()
        self.send_response(status)
        self.send_header("Content-Type", "application/json")
        self.send_header("Cache-Control", "no-store")
        for name, value in extra_headers:
            self.send_header(name, value)
        for cookie in cookies:
            self.send_header("Set-Cookie", cookie)
        self.send_header("Content-Length", str(len(data)))
        self.end_headers()
        if getattr(self, 'command', None) != 'HEAD':
            self.wfile.write(data)

    def authorized(self, host):
        if self.path == '/__momso/ready' and self.command in ('GET', 'HEAD'):
            body = b'{"ready":true}'
            self.send_response(200)
            self.send_header('Content-Type', 'application/json')
            self.send_header('Cache-Control', 'no-store')
            self.send_header('Access-Control-Allow-Origin', '*')
            self.send_header('Content-Length', str(len(body)))
            self.end_headers()
            if self.command == 'GET': self.wfile.write(body)
            return False
        if self.command in ("GET", "HEAD") and self.path in PUBLIC_ASSETS:
            name, content_type = PUBLIC_ASSETS[self.path]
            try:
                root = Path(os.environ.get("MOMSO_PUBLIC_ASSETS_DIR", str(Path(__file__).resolve().parent / "public"))).resolve()
                path = (root / name).resolve()
                if not path.is_relative_to(root) or not path.is_file():
                    raise OSError("missing public asset")
                data = path.read_bytes()
                self.send_response(200)
                self.send_header("Content-Type", content_type)
                self.send_header("Cache-Control", "public, max-age=3600")
                self.send_header("Content-Length", str(len(data)))
                self.end_headers()
                if self.command == "GET":
                    self.wfile.write(data)
            except OSError:
                self.send_error(404)
            return False
        if self.path == "/__momso/auth/return" and self.command == "GET":
            body = b'<!doctype html><meta charset="utf-8"><script>' + RETURN_SCRIPT + b'</script>'
            script_hash = __import__('base64').b64encode(hashlib.sha256(RETURN_SCRIPT).digest()).decode('ascii')
            self.send_response(200)
            self.send_header("Content-Type", "text/html; charset=utf-8")
            self.send_header("Cache-Control", "no-store")
            self.send_header("Referrer-Policy", "no-referrer")
            self.send_header("Content-Security-Policy", "default-src 'none'; script-src 'sha256-" + script_hash + "'; connect-src 'self'; base-uri 'none'; frame-ancestors 'none'")
            self.send_header("Content-Length", str(len(body)))
            self.end_headers()
            self.wfile.write(body)
            return False
        cookies = SimpleCookie()
        try:
            cookies.load(self.headers.get("Cookie", ""))
        except Exception:
            self.json_reply(401, {"error": "login_required"})
            return False
        if self.path == "/__momso/login.mjs" and self.command == "GET":
            try:
                data = Path(__file__).with_name("momso_anywhere_login.mjs").read_bytes()
                self.send_response(200)
                self.send_header("Content-Type", "text/javascript; charset=utf-8")
                self.send_header("Cache-Control", "no-store")
                self.send_header("Content-Length", str(len(data)))
                self.end_headers()
                self.wfile.write(data)
            except OSError:
                self.send_error(503)
            return False
        if self.path == "/__momso/auth/challenge" and self.command == "GET":
            state = auth.challenge("https://" + host)
            self.json_reply(200, {"state": state}, ("__Host-momso-challenge=" + state + "; Secure; HttpOnly; SameSite=Lax; Path=/; Max-Age=300",))
            return False
        if self.path == "/__momso/auth/finish" and self.command == "POST":
            try:
                length = int(self.headers.get("Content-Length", "0"))
                if not 0 < length <= 65536:
                    raise ValueError("invalid request")
                body = json.loads(self.rfile.read(length))
                if self.headers.get("Origin") != "https://" + host:
                    self.json_reply(403, {'error': 'origin_not_allowed'})
                    return False
                state_cookie = cookies.get("__Host-momso-challenge")
                session = auth.finish(body["state"], state_cookie.value if state_cookie else "", body["credential"], "https://" + host)
                self.json_reply(200, {"ok": True}, ("__Host-momso-session=" + session + "; Secure; HttpOnly; SameSite=Strict; Path=/; Max-Age=" + str(auth.SESSION_SECONDS), "__Host-momso-challenge=; Secure; HttpOnly; SameSite=Strict; Path=/; Max-Age=0"))
            except auth.AuthenticationError as error:
                self.json_reply(403, {"error": error.code})
            except Exception:
                self.json_reply(403, {"error": "login_failed"})
            return False
        cookie = cookies.get("__Host-momso-session")
        if not cookie or not auth.valid_session(cookie.value):
            if self.path in PAUSE_OWNER:
                try:
                    length = int(self.headers.get('Content-Length', '0'))
                    if 0 < length <= 1024:
                        self.rfile.read(length)
                except ValueError:
                    pass
                self.json_reply(401, {'error': auth.session_rejection(cookie.value) if cookie else 'pause_requires_app_session'})
                return False
            if self.command == "GET" and self.headers.get("Sec-Fetch-Mode") == "navigate" and self.headers.get("Sec-Fetch-Dest") == "document":
                try:
                    origin = login_origin()
                except ValueError:
                    self.send_error(503)
                    return False
                state = auth.challenge("https://" + host)
                params = {"state": state}
                if urllib.parse.parse_qs(urllib.parse.urlsplit(self.path).query).get("auth_error") == ["1"]:
                    params["error"] = "1"
                destination = origin + "/?" + urllib.parse.urlencode(params)
                script = "location.replace(" + json.dumps(destination) + ");"
                script_hash = base64.b64encode(hashlib.sha256(script.encode()).digest()).decode()
                body = ('<!doctype html><meta charset="utf-8"><meta http-equiv="refresh" content="0;url=' + html.escape(destination, quote=True) + '"><script>' + script + '</script>').encode()
                self.send_response(200)
                self.send_header("Content-Type", "text/html; charset=utf-8")
                self.send_header("Content-Security-Policy", "default-src 'none'; script-src 'sha256-" + script_hash + "'; base-uri 'none'")
                self.send_header("Set-Cookie", "__Host-momso-challenge=" + state + "; Secure; HttpOnly; SameSite=Lax; Path=/; Max-Age=300")
                self.send_header("Cache-Control", "no-store")
                self.send_header("Referrer-Policy", "strict-origin")
                self.send_header("Content-Length", str(len(body)))
                self.end_headers()
                self.wfile.write(body)
                return False
            self.json_reply(401, {"error": auth.session_rejection(cookie.value) if cookie else "login_required"})
            return False
        return True

    def handle_request(self):
        response_started = False
        if self.headers.get("Transfer-Encoding"):
            self.send_error(411)
            return
        host = self.headers.get("Host", "")
        if not host or host.lower() != os.environ.get("MOMSO_PUBLIC_HOST", "").lower() or any(c not in "abcdefghijklmnopqrstuvwxyz0123456789.-" for c in host.lower()):
            self.json_reply(403, {'error': 'invalid_host'})
            return
        ticket_request = self.path in PAUSE_TICKET
        if self.path.startswith('/__momso/pause/') and not ticket_request and self.path not in PAUSE_OWNER:
            self.send_error(404)
            return
        if ticket_request:
            try:
                allowed_origin = login_origin()
            except ValueError:
                self.send_error(503)
                return
            if self.headers.get('Origin') != allowed_origin:
                self.json_reply(403, {'error': 'origin_not_allowed'})
                return
            self._pause_cors_origin = allowed_origin
            if self.command == 'OPTIONS':
                if self.headers.get('Access-Control-Request-Method') != 'POST' or self.headers.get('Access-Control-Request-Headers', '').lower().strip() not in ('', 'content-type'):
                    self.json_reply(403, {'error': 'preflight_not_allowed'})
                    return
                self.send_response(204)
                self.send_header('Access-Control-Allow-Methods', 'POST')
                self.send_header('Access-Control-Allow-Headers', 'Content-Type')
                self.send_header('Cache-Control', 'no-store')
                self.send_header('Content-Length', '0')
                self.end_headers()
                return
            if self.command != 'POST':
                self.send_error(405)
                return
        elif not self.authorized(host):
            return
        if self.path in PAUSE_OWNER and self.command != PAUSE_OWNER[self.path]:
            self.send_error(405)
            return
        if self.path in PAUSE_OWNER and (self.headers.get('Origin') not in (None, 'https://' + host) or self.headers.get('Sec-Fetch-Site') not in (None, 'same-origin')):
            self.json_reply(403, {'error': 'origin_not_allowed'})
            return
        if not ticket_request and self.command not in ("GET", "HEAD") and self.headers.get("Origin") != "https://" + host:
            self.json_reply(403, {'error': 'origin_not_allowed'})
            return
        if self.command == "POST" and self.path == "/__momso/auth/logout":
            self.json_reply(200, {"ok": True}, ("__Host-momso-session=; Secure; HttpOnly; SameSite=Strict; Path=/; Max-Age=0",))
            return
        try:
            try:
                length = int(self.headers.get("Content-Length", "0"))
            except ValueError:
                self.json_reply(400, {'error': 'invalid_request'})
                return
            if length < 0 or length > (1024 if self.path.startswith('/__momso/pause/') else 32 * 1024 * 1024):
                self.json_reply(400, {'error': 'invalid_request'})
                return
            token = os.environ["MOMSO_PRIVATE_PROXY_TOKEN"]
            if not token or "\n" in token or "\r" in token:
                raise ValueError("invalid token")
            headers = {k: v for k, v in self.headers.items() if k.lower() not in HOP | {"authorization", "cookie", "host", "x-serverless-authorization", "x-goog-iap-jwt-assertion"}}
            headers["Authorization"] = "Bearer " + token
            body = self.rfile.read(length) if length else None
            navigation = self.command == "GET" and self.headers.get("Sec-Fetch-Mode") == "navigate" and self.headers.get("Sec-Fetch-Dest") == "document"
            connection = None
            try:
                connection = upstream_connection()
                connection.connect()
                connection.sock.settimeout(None)
                connection.request(self.command, self.path, body=body, headers=headers)
                response = connection.getresponse()
            except ssl.SSLError:
                if connection:
                    connection.close()
                raise
            except (OSError, http.client.HTTPException):
                if connection:
                    connection.close()
                if ticket_request:
                    raise
                request_wake()
                if not navigation:
                    self.json_reply(503, {'error': 'waking', 'retryAfterSeconds': WAKE_RETRY_SECONDS}, extra_headers=(('Retry-After', str(WAKE_RETRY_SECONDS)),))
                    return
                body = '<!doctype html><html lang="ko"><meta charset="utf-8"><meta name="viewport" content="width=device-width,initial-scale=1"><meta http-equiv="refresh" content="5"><title>momso</title><body><p>몸소를 켜고 있습니다.</p></body></html>'.encode()
                self.send_response(200)
                self.send_header("Content-Type", "text/html; charset=utf-8")
                self.send_header("Cache-Control", "no-store")
                self.send_header("Content-Length", str(len(body)))
                self.end_headers()
                self.wfile.write(body)
                return
            self._response_source = 'upstream'
            self.send_response(response.status)
            response_started = True
            for key, value in response.getheaders():
                if key.lower() not in HOP | {"cache-control", "expires", "pragma", "set-cookie", "x-momso-response", "access-control-allow-origin", "access-control-expose-headers"}:
                    self.send_header(key, value)
            self.send_header("Cache-Control", "no-store")
            self.end_headers()
            # Long-running SSE has no read timeout; the Cloud Run request timeout still applies.
            while True:
                chunk = response.read1(8192)
                if not chunk:
                    break
                self.wfile.write(chunk)
                self.wfile.flush()
            connection.close()
        except (OSError, ValueError, KeyError, http.client.HTTPException):
            if not response_started:
                if self.path.startswith('/__momso/pause/'):
                    self.json_reply(503, {'error': 'vm_unreachable'})
                else:
                    self.send_error(503, "momso is not ready")
            self.close_connection = True

    do_GET = handle_request
    do_HEAD = handle_request
    do_POST = handle_request
    do_PUT = handle_request
    do_PATCH = handle_request
    do_DELETE = handle_request
    do_OPTIONS = handle_request


if __name__ == "__main__":
    http.server.ThreadingHTTPServer(("0.0.0.0", int(os.environ.get("PORT", "8080"))), Handler).serve_forever()
