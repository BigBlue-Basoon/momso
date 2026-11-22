"""TLS endpoint on the private VM, forwarding only authenticated gateway requests."""
import hmac
import http.server
import os
import ssl
from pathlib import Path

import momso_mobile_proxy


class Handler(momso_mobile_proxy.Handler):
    def handle_request(self):
        try:
            expected = Path(os.environ["MOMSO_PRIVATE_PROXY_TOKEN_FILE"]).read_text(encoding="utf-8").strip()
            provided = self.headers.get("Authorization", "")
            if not expected or not hmac.compare_digest(provided, "Bearer " + expected):
                self.send_error(403)
                return
            del self.headers["Authorization"]
            host = os.environ["MOMSO_MOBILE_HOST"]
            if "Host" in self.headers:
                self.headers.replace_header("Host", host)
            else:
                self.headers["Host"] = host
            if self.command not in ("GET", "HEAD"):
                if "Origin" in self.headers:
                    self.headers.replace_header("Origin", "https://" + host)
                else:
                    self.headers["Origin"] = "https://" + host
            super().handle_request()
        except (OSError, KeyError, ValueError):
            self.send_error(503)

    do_GET = handle_request
    do_HEAD = handle_request
    do_POST = handle_request
    do_PUT = handle_request
    do_PATCH = handle_request
    do_DELETE = handle_request


def main():
    port = int(os.environ.get("MOMSO_PRIVATE_PORT", "8766"))
    server = http.server.ThreadingHTTPServer(("0.0.0.0", port), Handler)
    context = ssl.SSLContext(ssl.PROTOCOL_TLS_SERVER)
    context.load_cert_chain(os.environ["MOMSO_PRIVATE_CERT"], os.environ["MOMSO_PRIVATE_KEY"])
    server.socket = context.wrap_socket(server.socket, server_side=True)
    server.serve_forever()


if __name__ == "__main__":
    main()
