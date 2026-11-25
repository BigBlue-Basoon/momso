"""Isolated Linux gateway measurement against a fake local server, no cloud API."""
import argparse
import hashlib
import http.client
import http.server
import json
import os
import resource
import tempfile
import threading
import time
import types
from pathlib import Path

import momso_anywhere_auth as auth
import momso_anywhere_gateway as gateway
import momso_anywhere_vm_proxy as vm_proxy
import momso_mobile_proxy as mobile_proxy


class FakeServer(http.server.BaseHTTPRequestHandler):
    def log_message(self, *_): pass

    def do_POST(self):
        left = int(self.headers['Content-Length'])
        digest = hashlib.sha256()
        while left:
            chunk = self.rfile.read(min(left, 65536))
            if not chunk: break
            digest.update(chunk)
            left -= len(chunk)
        expected = self.headers.get('X-Momso-Chunk-Sha256')
        self.send_response(200 if not left and (not expected or expected == digest.hexdigest()) else 400)
        self.end_headers()
        self.wfile.write(b'ok')

    do_PUT = do_POST

    def do_GET(self):
        self.send_response(200)
        self.send_header('Content-Type', 'text/event-stream')
        self.end_headers()
        deadline = time.monotonic() + self.server.duration
        while time.monotonic() < deadline:
            self.wfile.write(b'data: tick\n\n')
            self.wfile.flush()
            time.sleep(min(10, max(0, deadline - time.monotonic())))
        self.wfile.write(b'data: done\n\n')
        self.wfile.flush()


def main():
    parser = argparse.ArgumentParser()
    parser.add_argument('--duration', type=int, default=1800)
    parser.add_argument('--chunks', action='store_true')
    args = parser.parse_args()
    with tempfile.TemporaryDirectory(prefix='momso-s170-') as tmp:
        key = Path(tmp) / 'session-key'
        key.write_bytes(b'fixture-only-session-key-32-bytes-12345')
        proxy_token = Path(tmp) / 'proxy-token'
        proxy_token.write_text('fixture')
        (Path(tmp) / 'server-env').write_text('MOMSO_SERVER_TOKEN=fixture-server-token\n')
        os.environ.update(MOMSO_PUBLIC_HOST='fixture.run.app', MOMSO_PRIVATE_PROXY_TOKEN='fixture',
                          MOMSO_SESSION_KEY_FILE=str(key), MOMSO_OWNER_SUB='fixture-owner',
                          MOMSO_PRIVATE_PROXY_TOKEN_FILE=str(proxy_token), MOMSO_MOBILE_HOST='fixture.run.app',
                          CREDENTIALS_DIRECTORY=tmp)
        cookie = auth._sign('session', {'iat': int(time.time()), 'sub': 'fixture-owner'})
        upstream = http.server.ThreadingHTTPServer(('127.0.0.1', 0), FakeServer)
        upstream.duration = args.duration
        proxy = http.server.ThreadingHTTPServer(('127.0.0.1', 0), vm_proxy.Handler)
        entry = http.server.ThreadingHTTPServer(('127.0.0.1', 0), gateway.Handler)
        connection = http.client.HTTPConnection
        mobile_proxy.http = types.SimpleNamespace(client=types.SimpleNamespace(
            HTTPConnection=lambda _host, _port: connection('127.0.0.1', upstream.server_address[1])))
        gateway.upstream_connection = lambda: connection('127.0.0.1', proxy.server_address[1], timeout=10)
        for server in (upstream, proxy, entry):
            threading.Thread(target=server.serve_forever, daemon=True).start()
        headers = {'Host': 'fixture.run.app', 'Origin': 'https://fixture.run.app', 'Cookie': '__Host-momso-session=' + cookie}
        try:
            for size in (10, 40, 200, 1024):
                conn = http.client.HTTPConnection('127.0.0.1', entry.server_address[1], timeout=120)
                count = size * 1024 * 1024
                started = time.monotonic()
                if count <= 32 * 1024 * 1024:
                    block = b'x' * 65536
                    conn.request('POST', '/uploads', body=(block for _ in range(count // len(block))), headers={**headers, 'Content-Length': str(count)})
                else:
                    # The gateway rejects the declared length before reading a body.
                    conn.putrequest('POST', '/uploads', skip_host=True)
                    for name, value in {**headers, 'Content-Length': str(count)}.items(): conn.putheader(name, value)
                    conn.endheaders()
                response = conn.getresponse()
                body = response.read(512).decode('utf-8', 'replace')
                print(json.dumps({'mib': size, 'status': response.status, 'body': body,
                                  'seconds': round(time.monotonic()-started, 3),
                                  'process_peak_rss_kib': resource.getrusage(resource.RUSAGE_SELF).ru_maxrss,
                                  'body_sent': count <= 32 * 1024 * 1024}), flush=True)
                conn.close()
            if args.chunks:
                block = b'x' * 65536
                for size in (10, 40, 200, 1024):
                    remaining = size * 1024 * 1024
                    count = 0
                    started = time.monotonic()
                    while remaining:
                        length = min(8 * 1024 * 1024, remaining)
                        digest = hashlib.sha256()
                        for _ in range(length // len(block)): digest.update(block)
                        conn = http.client.HTTPConnection('127.0.0.1', entry.server_address[1], timeout=120)
                        conn.request('PUT', '/uploads/fixture/chunks?offset=' + str(count),
                                     body=(block for _ in range(length // len(block))),
                                     headers={**headers, 'Content-Length': str(length), 'X-Momso-Chunk-Sha256': digest.hexdigest()})
                        response = conn.getresponse()
                        if response.status != 200: raise RuntimeError('chunk rejected: ' + str(response.status))
                        response.read(); conn.close()
                        remaining -= length; count += length
                    print(json.dumps({'chunked_mib': size, 'bytes_sent': count,
                                      'seconds': round(time.monotonic()-started, 3),
                                      'process_peak_rss_kib': resource.getrusage(resource.RUSAGE_SELF).ru_maxrss}), flush=True)
            conn = http.client.HTTPConnection('127.0.0.1', entry.server_address[1], timeout=args.duration + 30)
            started = time.monotonic()
            conn.request('GET', '/events', headers=headers)
            response = conn.getresponse()
            received = 0
            while True:
                line = response.readline()
                if not line: break
                if line.startswith(b'data:'): received += 1
                if line == b'data: done\n': break
            print(json.dumps({'sse_status': response.status, 'seconds': round(time.monotonic()-started, 1), 'events': received, 'finished': line == b'data: done\n'}), flush=True)
            conn.close()
        finally:
            entry.shutdown(); proxy.shutdown(); upstream.shutdown()
            entry.server_close(); proxy.server_close(); upstream.server_close()


if __name__ == '__main__':
    main()
