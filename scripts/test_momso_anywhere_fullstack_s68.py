"""Opt-in local gateway/proxy/server integration using an isolated fixture bundle."""
import base64
import hashlib
import http.client
import http.server
import json
import os
import shutil
import socket
import ssl
import subprocess
import tarfile
import tempfile
import threading
import time
import unittest
from pathlib import Path
from unittest.mock import patch

import momso_anywhere_auth as auth
import momso_anywhere_gateway as gateway
import momso_anywhere_install
import momso_anywhere_vm_proxy as private
import momso_server_install as installer


class FullStack(unittest.TestCase):
    def test_real_gateway_proxy_and_server(self):
        artifacts = Path(os.environ['MOMSO_S68_ARTIFACT_DIR'])
        expected = dict(line.split()[::-1] for line in (artifacts / 'SHA256SUMS').read_text().splitlines())
        for name in ('momso-server', 'web.tar.gz'):
            self.assertEqual(hashlib.sha256((artifacts / name).read_bytes()).hexdigest(), expected[name])
        with tempfile.TemporaryDirectory(prefix='momso-s68-') as tmp:
            root = Path(tmp)
            web = root / 'web'
            web.mkdir()
            with tarfile.open(artifacts / 'web.tar.gz', 'r:gz') as archive:
                for member in archive.getmembers():
                    self.assertFalse(member.name.startswith('/') or '..' in Path(member.name).parts)
                    self.assertTrue(member.isfile() or member.isdir())
                archive.extractall(web, **({'filter': 'data'} if hasattr(tarfile, 'data_filter') else {}))
            bootstrap = os.environ.get('MOMSO_S68_BOOTSTRAP_OUT')
            if bootstrap:
                installed_web = Path(bootstrap) / 'web'
                self.assertEqual((installed_web / 'index.html').read_bytes(), (web / 'index.html').read_bytes())
                shutil.rmtree(web)
                shutil.copytree(installed_web, web)
            codex = root / 'codex'
            codex.write_text('''#!/bin/sh
while IFS= read -r line; do
 case "$line" in
 *'"id":1'*) printf '%s\\n' '{"id":1,"result":{}}' ;;
 *'"id":2'*) printf '%s\\n' '{"id":2,"result":{"thread":{"id":"fixture-thread"}}}' ;;
 *'"id":3'*) printf '%s\\n' '{"id":3,"result":{}}' '{"method":"item/agentMessage/delta","params":{"delta":"fixture reply"}}' '{"method":"turn/completed","params":{"turn":{"status":"completed"}}}' ;;
 esac
done
''')
            codex.chmod(0o700)
            credentials = root / 'credentials'
            credentials.mkdir()
            (credentials / 'server-env').write_text('MOMSO_SERVER_TOKEN=fixture-server-token\n')
            subprocess.run(['age-keygen', '-o', str(credentials / 'momso-vault-identity')], check=True, stdout=subprocess.DEVNULL, stderr=subprocess.DEVNULL)
            identity = credentials / 'momso-vault-identity'
            identity.write_text(next(line for line in identity.read_text().splitlines() if line.startswith('AGE-SECRET-KEY-')))
            (root / 'transport').write_text('fixture-transport-token')
            (root / 'session').write_text('s' * 48)
            cert, key = root / 'cert.pem', root / 'key.pem'
            subprocess.run(['openssl', 'req', '-x509', '-newkey', 'rsa:2048', '-nodes', '-days', '1', '-subj', '/CN=fixture', '-keyout', str(key), '-out', str(cert)], check=True, stdout=subprocess.DEVNULL, stderr=subprocess.DEVNULL)
            fingerprint = hashlib.sha256(ssl.PEM_cert_to_DER_cert(cert.read_text())).hexdigest()
            # The production VM proxy currently forwards to 8765; do not touch an existing service.
            with socket.socket() as probe:
                if probe.connect_ex(('127.0.0.1', 8765)) == 0:
                    self.fail('127.0.0.1:8765 already in use; retry in an isolated environment')
            env = dict(os.environ, MOMSO_SERVER_TOKEN='fixture-server-token', MOMSO_SERVER_DATA_DIR=str(root / 'data'), MOMSO_SERVER_WEB_DIR=str(web), MOMSO_SERVER_LISTEN='127.0.0.1:8765', MOMSO_CODEX_PATH=str(codex), CREDENTIALS_DIRECTORY=str(credentials))
            installed = root / 'momso-server'
            source_server = Path(bootstrap) / 'momso-server' if bootstrap else artifacts / 'momso-server'
            self.assertEqual(hashlib.sha256(source_server.read_bytes()).hexdigest(), expected['momso-server'])
            shutil.copy2(source_server, installed)
            server = [subprocess.Popen([str(installed)], env=env, stdout=subprocess.DEVNULL, stderr=subprocess.DEVNULL)]
            def restart(artifact):
                server[0].terminate()
                server[0].wait(timeout=10)
                shutil.copy2(artifact, installed)
                server[0] = subprocess.Popen([str(installed)], env=env, stdout=subprocess.DEVNULL, stderr=subprocess.DEVNULL)
            try:
                    for _ in range(100):
                        try:
                            c = http.client.HTTPConnection('127.0.0.1', 8765, timeout=1)
                            c.request('GET', '/health', headers={'Authorization': 'Bearer fixture-server-token'})
                            if c.getresponse().status == 200:
                                c.close()
                                break
                        except OSError:
                            time.sleep(.1)
                    else:
                        self.fail('server did not become ready')
                    self.exercise(root, cert, key, fingerprint, artifacts, restart)
            finally:
                server[0].terminate()
                server[0].wait(timeout=10)

    def exercise(self, root, cert, key, fingerprint, artifacts, restart):
        proxy = http.server.ThreadingHTTPServer(('127.0.0.1', 0), private.Handler)
        context = ssl.SSLContext(ssl.PROTOCOL_TLS_SERVER)
        context.load_cert_chain(cert, key)
        proxy.socket = context.wrap_socket(proxy.socket, server_side=True)
        entry = http.server.ThreadingHTTPServer(('127.0.0.1', 0), gateway.Handler)
        threads = [threading.Thread(target=s.serve_forever, daemon=True) for s in (proxy, entry)]
        for thread in threads:
            thread.start()
        variables = {'MOMSO_SESSION_KEY_FILE': str(root / 'session'), 'MOMSO_OWNER_SUB': 'fixture-owner', 'MOMSO_GOOGLE_CLIENT_ID': 'fixture-client', 'MOMSO_PUBLIC_HOST': 'fixture.run.app', 'MOMSO_PRIVATE_HOST': '127.0.0.1', 'MOMSO_PRIVATE_PORT': str(proxy.server_port), 'MOMSO_PRIVATE_CERT_SHA256': fingerprint, 'MOMSO_PRIVATE_PROXY_TOKEN': 'fixture-transport-token', 'MOMSO_PRIVATE_PROXY_TOKEN_FILE': str(root / 'transport'), 'MOMSO_MOBILE_HOST': 'private.momso', 'CREDENTIALS_DIRECTORY': str(root / 'credentials')}
        def request(method, path, body=None, cookie=None):
            conn = http.client.HTTPConnection('127.0.0.1', entry.server_port, timeout=10)
            headers = {'Host': 'fixture.run.app', 'Origin': 'https://fixture.run.app'}
            if cookie: headers['Cookie'] = cookie
            if body is not None: headers['Content-Type'] = 'application/json'
            conn.request(method, path, json.dumps(body) if body is not None else None, headers)
            res = conn.getresponse()
            data, headers = res.read(), res.getheaders()
            status = res.status
            conn.close()
            return status, data, headers
        try:
            with patch.dict(os.environ, variables):
                status, raw, headers = request('GET', '/__momso/auth/challenge')
                self.assertEqual(status, 200)
                state = json.loads(raw)['state']
                challenge = next(v.split(';')[0] for k, v in headers if k.lower() == 'set-cookie')
                nonce = base64.urlsafe_b64encode(hashlib.sha256(state.encode()).digest()).rstrip(b'=').decode()
                real_finish = auth.finish
                def fake_google(s, c, credential, origin):
                    return real_finish(s, c, credential, origin, verifier=lambda token, client: {'sub': 'fixture-owner', 'nonce': nonce} if token == 'fixture-id-token' else {})
                with patch.object(auth, 'finish', side_effect=fake_google):
                    status, _, headers = request('POST', '/__momso/auth/finish', {'state': state, 'credential': 'fixture-id-token'}, challenge)
                self.assertEqual(status, 200)
                session = next(v.split(';')[0] for k, v in headers if k.lower() == 'set-cookie' and v.startswith('__Host-momso-session='))
                for path in ('/', '/health', '/providers', '/chats', '/documents', '/saved-logins'):
                    with self.subTest(path=path):
                        status, raw, _ = request('GET', path, cookie=session)
                        self.assertEqual(status, 200, (path, status, raw[:200]))
                with self.subTest(path='fake_codex_turn'):
                    status, raw, _ = request('POST', '/chats', {'title': 'fixture'}, session)
                    self.assertIn(status, (200, 201), raw[:200])
                    chat_id = json.loads(raw)['chatId']
                    status, raw, _ = request('POST', f'/chats/{chat_id}/turns', {'text': 'fixture prompt', 'provider': 'codex'}, session)
                    self.assertIn(status, (200, 201, 202), raw[:200])
                    turn_id = json.loads(raw)['turnId']
                    for _ in range(100):
                        status, raw, _ = request('GET', f'/turns/{turn_id}', cookie=session)
                        self.assertEqual(status, 200)
                        turn = json.loads(raw)
                        if turn.get('state') not in ('running', 'pending'):
                            break
                        time.sleep(.1)
                    self.assertEqual(turn.get('state'), 'completed', turn)
                    self.assertEqual(turn.get('text'), 'fixture reply')
                with self.subTest(path='upgrade_and_read'):
                    newer = root / 'new-server'
                    shutil.copy2(artifacts / 'momso-server', newer)
                    with newer.open('ab') as file: file.write(b'\x00fixture-version')
                    newer_web = root / 'new-web.tar.gz'
                    with tarfile.open(artifacts / 'web.tar.gz', 'r:gz') as old, tarfile.open(newer_web, 'w:gz') as new:
                        for item in old.getmembers(): new.addfile(item, old.extractfile(item) if item.isfile() else None)
                        info = tarfile.TarInfo('s68-fixture-version.txt')
                        info.size = 2
                        import io
                        new.addfile(info, io.BytesIO(b'v2'))
                    class LocalProvider:
                        install_id, name, network = 'a' * 24, 'fixture-vm', 'fixture-net'
                        project = 'fixture-project'
                        def __init__(self):
                            self.image = None
                            self.traffic = 'fixture-old'
                        def run(self, *args, **kwargs):
                            if args[:3] == ('compute', 'instances', 'describe'):
                                return {'labels': {'momso-install-id': self.install_id}, 'networkInterfaces': [{'network': '/networks/fixture-net'}], 'status': 'RUNNING'}
                            if args[:2] == ('run', 'deploy'):
                                self.image = args[args.index('--image') + 1]
                                return b''
                            if args[:3] == ('run', 'services', 'update-traffic'):
                                self.traffic = args[args.index('--to-revisions') + 1].split('=')[0]
                                return b''
                            value = {'status': {'url': 'https://fixture.run.app', 'traffic': [{'revisionName': self.traffic, 'percent': 100}], 'latestReadyRevisionName': 'fixture-new', 'conditions': [{'type': 'Ready', 'status': 'True'}]}}
                            return value if kwargs.get('parse') else json.dumps(value).encode()
                        def upgrade_checkpoint(self, action): pass
                        def ensure_private_proxy(self): pass
                        def upgrade_server(self, artifact, digest): restart(artifact)
                        def install_web(self, archive, digest):
                            with tarfile.open(archive, 'r:gz') as files: files.extractall(root / 'web', **({'filter': 'data'} if hasattr(tarfile, 'data_filter') else {}))
                        def health(self):
                            for _ in range(100):
                                try:
                                    conn = http.client.HTTPConnection('127.0.0.1', 8765, timeout=1)
                                    conn.request('GET', '/health', headers={'Authorization': 'Bearer fixture-server-token'})
                                    if conn.getresponse().status == 200:
                                        conn.close()
                                        return
                                except OSError: time.sleep(.1)
                            raise installer.InstallError('fixture server did not restart')
                    state = {'anywhere': {'url': 'https://fixture.run.app', 'client_id': 'fixture-client', 'owner_sub': 'fixture-owner', 'login_origin': 'https://login.example.test'}}
                    digest = lambda p: hashlib.sha256(p.read_bytes()).hexdigest()
                    provider = LocalProvider()
                    image = 'registry.example.test/momso/gateway@sha256:' + 'a' * 64
                    with patch.object(momso_anywhere_install.GoogleAnywhere, '_objects', return_value=({'fixture': True}, {'fixture': True}, {'fixture': True})), patch('momso_anywhere_vm_services.install'):
                        installer.upgrade_anywhere(provider, state, newer, digest(newer), newer_web, digest(newer_web), 'fixture-client', 'fixture-owner', 'https://login.example.test', image)
                    self.assertEqual((provider.image, provider.traffic), (image, 'fixture-new'))
                    status, raw, _ = request('GET', f'/chats/{chat_id}', cookie=session)
                    self.assertEqual(status, 200, raw[:200])
                    self.assertIn(b'fixture reply', raw)
                    status, raw, _ = request('GET', '/s68-fixture-version.txt', cookie=session)
                    self.assertEqual((status, raw), (200, b'v2'))
        finally:
            for server in (entry, proxy): server.shutdown(); server.server_close()
            for thread in threads: thread.join(2)


if __name__ == '__main__': unittest.main()
