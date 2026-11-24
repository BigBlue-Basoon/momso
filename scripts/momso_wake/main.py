"""Single-VM wake endpoint; deploy only behind Cloud Run IAP."""
import html
import http.server
import json
import os
import urllib.error
import urllib.parse
import urllib.request


def api_request(url, *, method="GET", token=None, metadata=False):
    headers = {"Metadata-Flavor": "Google"} if metadata else {"Authorization": "Bearer " + token}
    request = urllib.request.Request(url, headers=headers, method=method, data=b"" if method == "POST" else None)
    with urllib.request.urlopen(request, timeout=20) as response:
        return json.load(response)


def wake():
    project, zone, instance = (os.environ[k] for k in ("MOMSO_PROJECT", "MOMSO_ZONE", "MOMSO_INSTANCE"))
    base = "https://compute.googleapis.com/compute/v1/projects/" + "/zones/".join((urllib.parse.quote(project, safe=""), urllib.parse.quote(zone, safe=""))) + "/instances/" + urllib.parse.quote(instance, safe="")
    token = api_request("http://metadata.google.internal/computeMetadata/v1/instance/service-accounts/default/token", metadata=True)["access_token"]
    current = api_request(base, token=token).get("status")
    if current in ("RUNNING", "PROVISIONING", "STAGING", "STARTING"):
        return current
    api_request(base + "/start", method="POST", token=token)
    return "STARTING"


class Handler(http.server.BaseHTTPRequestHandler):
    def log_message(self, *_args):
        pass

    def reply(self, status, text):
        body = text.encode("utf-8")
        self.send_response(status)
        self.send_header("Content-Type", "text/html; charset=utf-8")
        self.send_header("Cache-Control", "no-store")
        self.send_header("Content-Security-Policy", "default-src 'none'; form-action 'self'; base-uri 'none'")
        self.send_header("Content-Length", str(len(body)))
        self.end_headers()
        self.wfile.write(body)

    def do_GET(self):
        if self.path != "/":
            self.reply(404, "Not found")
            return
        self.reply(200, '<form method="post" action="/wake"><button>몸소 켜기</button></form>')

    def do_POST(self):
        host = self.headers.get("Host", "")
        if self.path != "/wake" or not host or self.headers.get("Origin") != "https://" + host:
            self.reply(403, "요청을 확인할 수 없습니다.")
            return
        try:
            wake()
            hostname = os.environ["MOMSO_MOBILE_HOST"]
            if not hostname or any(c not in "abcdefghijklmnopqrstuvwxyz0123456789.-" for c in hostname):
                raise ValueError("invalid hostname")
            link = html.escape("https://" + hostname, quote=True)
            self.reply(200, '몸소를 켜고 있습니다. 잠시 뒤 <a href="' + link + '">몸소 열기</a>를 누르세요.')
        except (OSError, ValueError, KeyError, urllib.error.URLError):
            self.reply(503, "몸소를 켜지 못했습니다. 잠시 뒤 다시 시도하세요.")


if __name__ == "__main__":
    http.server.ThreadingHTTPServer(("0.0.0.0", int(os.environ.get("PORT", "8080"))), Handler).serve_forever()
