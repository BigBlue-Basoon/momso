"""Exercise actual server and both HTTP proxy handlers with disposable fixtures."""
import argparse
import hashlib
import http.client
import http.server
import json
import os
import socket
import subprocess
import tempfile
import threading
import time
import types
from pathlib import Path

import momso_anywhere_auth as auth
import momso_anywhere_gateway as gateway
import momso_anywhere_vm_proxy as vm_proxy
import momso_mobile_proxy as mobile_proxy


def unused_port():
    with socket.socket() as sock:
        sock.bind(('127.0.0.1', 0))
        return sock.getsockname()[1]


def main():
    parser = argparse.ArgumentParser()
    parser.add_argument('--server-binary', required=True)
    args = parser.parse_args()
    with tempfile.TemporaryDirectory(prefix='momso-s170-e2e-') as temporary:
        root = Path(temporary)
        (root / 'session').write_bytes(b'fixture-only-session-key-32-bytes-12345')
        (root / 'proxy-token').write_text('fixture')
        (root / 'server-env').write_text('MOMSO_SERVER_TOKEN=fixture-server-token\n')
        server_port = unused_port()
        os.environ.update(MOMSO_PUBLIC_HOST='fixture.run.app', MOMSO_PRIVATE_PROXY_TOKEN='fixture',
                          MOMSO_SESSION_KEY_FILE=str(root / 'session'), MOMSO_OWNER_SUB='fixture-owner',
                          MOMSO_PRIVATE_PROXY_TOKEN_FILE=str(root / 'proxy-token'), MOMSO_MOBILE_HOST='fixture.run.app',
                          CREDENTIALS_DIRECTORY=temporary)
        env = {**os.environ, 'MOMSO_SERVER_TOKEN': 'fixture-server-token', 'MOMSO_SERVER_DATA_DIR': str(root / 'data'),
               'MOMSO_SERVER_LISTEN': f'127.0.0.1:{server_port}', 'MOMSO_TEST_DISABLE_AUTO_BACKUP': '1'}
        env.pop('CREDENTIALS_DIRECTORY', None)
        process = subprocess.Popen([args.server_binary], env=env, stdout=subprocess.DEVNULL, stderr=subprocess.DEVNULL)
        proxy = entry = None
        try:
            for _ in range(200):
                if process.poll() is not None: raise RuntimeError('server exited')
                try:
                    with socket.create_connection(('127.0.0.1', server_port), timeout=.1): break
                except OSError: time.sleep(.05)
            else: raise RuntimeError('server not ready')
            proxy = http.server.ThreadingHTTPServer(('127.0.0.1', 0), vm_proxy.Handler)
            entry = http.server.ThreadingHTTPServer(('127.0.0.1', 0), gateway.Handler)
            connection = http.client.HTTPConnection
            mobile_proxy.http = types.SimpleNamespace(client=types.SimpleNamespace(HTTPConnection=lambda _host, _port: connection('127.0.0.1', server_port)))
            gateway.upstream_connection = lambda: connection('127.0.0.1', proxy.server_address[1], timeout=10)
            for server in (proxy, entry): threading.Thread(target=server.serve_forever, daemon=True).start()
            cookie = auth._sign('session', {'iat': int(time.time()), 'sub': 'fixture-owner'})
            base = {'Host': 'fixture.run.app', 'Origin': 'https://fixture.run.app', 'Cookie': '__Host-momso-session=' + cookie}

            def request(method, path, body, headers):
                client = connection('127.0.0.1', entry.server_address[1], timeout=180)
                client.request(method, path, body=body, headers={**base, **headers})
                response = client.getresponse()
                raw = response.read()
                client.close()
                if response.status != 200: raise RuntimeError(f'{path}: {response.status} {raw[:200]!r}')
                return json.loads(raw)

            chunk = b'x' * (4 * 1024 * 1024)
            chunk_hash = hashlib.sha256(chunk).hexdigest()
            for mib in (10, 40, 200, 1024):
                length = mib * 1024 * 1024
                full = hashlib.sha256()
                for offset in range(0, length, len(chunk)): full.update(chunk[:min(len(chunk), length-offset)])
                digest = full.hexdigest()
                body = json.dumps({'fileName': f'fixture-{mib}.bin', 'mediaType': 'application/octet-stream', 'byteLength': length, 'sha256': digest}).encode()
                start = time.monotonic()
                upload = request('POST', '/uploads', body, {'Content-Type': 'application/json'})['uploadId']
                for offset in range(0, length, len(chunk)):
                    part = chunk[:min(len(chunk), length-offset)]
                    result = request('PUT', f'/uploads/{upload}/chunks?offset={offset}', part,
                                     {'Content-Type': 'application/octet-stream', 'X-Momso-Chunk-Sha256': chunk_hash if len(part) == len(chunk) else hashlib.sha256(part).hexdigest()})
                    if result['received'] != offset + len(part): raise RuntimeError('wrong offset')
                asset = request('POST', f'/uploads/{upload}/finish', b'', {})
                if asset['sha256'] != digest or asset['byteLength'] != length: raise RuntimeError('digest mismatch')
                original = root / 'data/archive/objects/sha256' / digest[:2] / digest
                actual = hashlib.sha256()
                with original.open('rb') as stream:
                    while block := stream.read(1024 * 1024): actual.update(block)
                if actual.hexdigest() != digest: raise RuntimeError('stored original differs')
                print(json.dumps({'mib': mib, 'bytes': length, 'seconds': round(time.monotonic()-start, 2), 'stored_sha256_matches': True}), flush=True)
        finally:
            for server in (entry, proxy):
                if server: server.shutdown(); server.server_close()
            process.terminate()
            try: process.wait(timeout=10)
            except subprocess.TimeoutExpired: process.kill(); process.wait()


if __name__ == '__main__': main()
