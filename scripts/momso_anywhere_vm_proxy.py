"""TLS endpoint on the private VM, forwarding only authenticated gateway requests."""
import hmac
import http.server
import json
import os
import re
import ssl
from pathlib import Path

import momso_mobile_proxy
from momso_anywhere_vm_pause import create_vm_pause


class Handler(momso_mobile_proxy.Handler):
    def handle_request(self):
        try:
            expected = Path(os.environ["MOMSO_PRIVATE_PROXY_TOKEN_FILE"]).read_text(encoding="utf-8").strip()
            provided = self.headers.get("Authorization", "")
            if not expected or not hmac.compare_digest(provided, "Bearer " + expected):
                self.send_error(403)
                return
            if self.path.startswith('/__momso/pause/'):
                self.pause_request()
                return
            if self.command in ("GET", "HEAD") and self.path == "/update/status":
                self.update_status()
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

    def pause_request(self):
        operations = {
            '/__momso/pause/prepare': ('POST', 'prepare'),
            '/__momso/pause/status': ('GET', 'status'),
            '/__momso/pause/owner-release': ('POST', 'owner_release'),
            '/__momso/pause/reissue': ('POST', 'reissue'),
            '/__momso/pause/confirm': ('POST', 'confirm'),
            '/__momso/pause/release': ('POST', 'release'),
        }
        operation = operations.get(self.path)
        if operation is None or operation[0] != self.command:
            self.send_error(404)
            return
        try:
            length = int(self.headers.get('Content-Length', '0'))
            if not 0 <= length <= 1024:
                raise ValueError('invalid_request')
            body = json.loads(self.rfile.read(length)) if length else {}
            if not isinstance(body, dict):
                raise ValueError('invalid_request')
            tickets = self.server.pause_tickets
            action = operation[1]
            if action == 'owner_release':
                result = tickets.release(owner=True)
            elif action in ('confirm', 'release'):
                if set(body) != {'ticket'} or not isinstance(body['ticket'], str):
                    raise ValueError('invalid_request')
                result = getattr(tickets, action)(body['ticket'])
            else:
                if body:
                    raise ValueError('invalid_request')
                result = getattr(tickets, action)()
            reason = result.get('reason')
            if reason and reason not in ('backup_not_connected', 'invalid_pause_ticket', 'invalid_pause_state', 'maintenance_in_progress', 'maintenance_release_unconfirmed'):
                result['reason'] = 'backup_freshness_unverified'
            data = json.dumps(result, separators=(',', ':')).encode('utf-8')
            self.send_response(200 if result.get('ok', True) else 409)
            self.send_header('Content-Type', 'application/json')
            self.send_header('Cache-Control', 'no-store')
            self.send_header('Content-Length', str(len(data)))
            self.end_headers()
            self.wfile.write(data)
        except (ValueError, TypeError, OSError):
            self.send_error(400)

    def update_status(self):
        path = Path(os.environ.get("MOMSO_UPDATE_STATUS_FILE", "/run/momso-update/status.json"))
        try:
            with path.open("rb") as stream:
                raw = stream.read(4097)
            if len(raw) > 4096:
                raise ValueError("invalid_status")
            value = json.loads(raw)
            if not isinstance(value, dict):
                raise ValueError("invalid_status")
            states = {"checking", "up_to_date", "downloading", "waiting_for_idle", "backing_up",
                      "installing", "updating_gateway", "completed", "rolling_back", "rolled_back",
                      "recovery_required", "failed"}
            reason = value.get("reason")
            if (value.get("state") not in states or (reason is not None and
                    (not isinstance(reason, str) or not re.fullmatch(r"[a-z0-9_]{1,80}", reason)))):
                raise ValueError("invalid_status")
            detail = {}
            if 'code' in value:
                if (not isinstance(value['code'], str) or not re.fullmatch(r'gateway_[a-z0-9_]{1,72}', value['code']) or
                        any(value.get(key) is not None and type(value.get(key)) is not bool for key in ('rolledBack', 'retryable'))):
                    raise ValueError('invalid_status')
                detail = {key: value.get(key) for key in ('code', 'rolledBack', 'retryable')}
            body = json.dumps({"state": value["state"], "reason": reason, **detail}).encode("utf-8")
        except FileNotFoundError:
            body = b'{"state":"failed","reason":"update_unavailable"}'
        self.send_response(200)
        self.send_header("Content-Type", "application/json")
        self.send_header("Cache-Control", "no-store")
        self.send_header("Content-Length", str(len(body)))
        self.end_headers()
        if self.command != "HEAD":
            self.wfile.write(body)

    do_GET = handle_request
    do_HEAD = handle_request
    do_POST = handle_request
    do_PUT = handle_request
    do_PATCH = handle_request
    do_DELETE = handle_request


def main():
    port = int(os.environ.get("MOMSO_PRIVATE_PORT", "8766"))
    server = http.server.ThreadingHTTPServer(("0.0.0.0", port), Handler)
    server.pause_tickets = create_vm_pause()
    context = ssl.SSLContext(ssl.PROTOCOL_TLS_SERVER)
    context.load_cert_chain(os.environ["MOMSO_PRIVATE_CERT"], os.environ["MOMSO_PRIVATE_KEY"])
    server.socket = context.wrap_socket(server.socket, server_side=True)
    server.serve_forever()


if __name__ == "__main__":
    main()
