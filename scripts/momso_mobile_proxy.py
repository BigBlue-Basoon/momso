"""Local Unix-socket bridge from a validated Access tunnel to momso-server."""
import http.client
import http.server
import os
import socketserver
from pathlib import Path

SOCKET = "/run/momso-mobile/proxy.sock"
HOP = {"connection", "transfer-encoding", "keep-alive", "proxy-connection", "upgrade", "te", "trailer"}


def read_token(path=None):
    if path is None:
        path = Path(os.environ["CREDENTIALS_DIRECTORY"]) / "server-env"
    for line in Path(path).read_text(encoding="utf-8").splitlines():
        if line.startswith("MOMSO_SERVER_TOKEN="):
            token = line.partition("=")[2]
            if token and "\r" not in token and "\n" not in token:
                return token
    raise ValueError("missing server token")


class Handler(http.server.BaseHTTPRequestHandler):
    protocol_version = "HTTP/1.0"

    def log_message(self, _format, *_args):
        pass

    def handle_request(self):
        if self.headers.get("Transfer-Encoding"):
            self.send_error(411)
            return
        host = os.environ.get("MOMSO_MOBILE_HOST", "")
        if not host or self.headers.get("Host", "").lower() != host.lower():
            self.send_error(403)
            return
        if self.command not in ("GET", "HEAD") and self.headers.get("Origin") != "https://" + host:
            self.send_error(403)
            return
        try:
            length = int(self.headers.get("Content-Length", "0"))
            if length < 0:
                raise ValueError("invalid content length")
            token = read_token()
            headers = {key: value for key, value in self.headers.items() if key.lower() not in HOP | {"authorization", "cookie", "cf-access-jwt-assertion", "host"}}
            headers["Authorization"] = "Bearer " + token
            upstream = http.client.HTTPConnection("127.0.0.1", 8765)
            upstream.request(self.command, self.path, body=self.rfile.read(length) if length else None, headers=headers)
            response = upstream.getresponse()
            self.send_response(response.status)
            for key, value in response.getheaders():
                if key.lower() not in HOP | {"cache-control", "expires", "pragma"}:
                    self.send_header(key, value)
            self.send_header("Cache-Control", "no-store")
            self.end_headers()
            while True:
                chunk = response.read1(8192)
                if not chunk:
                    break
                self.wfile.write(chunk)
                self.wfile.flush()
            upstream.close()
        except (OSError, ValueError, KeyError, http.client.HTTPException):
            self.close_connection = True

    do_GET = handle_request
    do_POST = handle_request
    do_PUT = handle_request
    do_PATCH = handle_request
    do_DELETE = handle_request
    do_OPTIONS = handle_request


if hasattr(socketserver, "UnixStreamServer"):
    class Server(socketserver.ThreadingMixIn, socketserver.UnixStreamServer):
        daemon_threads = True
else:
    Server = None


def main():
    if Server is None:
        raise OSError("Unix sockets are unavailable")
    try:
        os.unlink(SOCKET)
    except FileNotFoundError:
        pass
    with Server(SOCKET, Handler) as server:
        os.chmod(SOCKET, 0o660)
        server.serve_forever()


if __name__ == "__main__":
    main()
