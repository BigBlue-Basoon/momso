import http.client
import http.server
import json
import os
from pathlib import Path
import tempfile
import threading
import time
import unittest
from unittest.mock import patch

import momso_anywhere_gateway as gateway
import momso_anywhere_vm_proxy as proxy
from momso_anywhere_pause_ticket import PauseTickets


class PauseGatewayTests(unittest.TestCase):
    def setUp(self):
        self.tmp = tempfile.TemporaryDirectory()
        self.addCleanup(self.tmp.cleanup)
        token = Path(self.tmp.name) / 'token'
        token.write_text('fixture-transport', encoding='utf-8')
        env = patch.dict(os.environ, {'MOMSO_PUBLIC_HOST': 'fixture.run.app', 'MOMSO_LOGIN_ORIGIN': 'https://login.example.test',
                                     'MOMSO_PRIVATE_PROXY_TOKEN': 'fixture-transport', 'MOMSO_PRIVATE_PROXY_TOKEN_FILE': str(token)})
        env.start(); self.addCleanup(env.stop)
        auth = patch.object(gateway.auth, 'valid_session', side_effect=lambda value: value == 'fixture-owner')
        auth.start(); self.addCleanup(auth.stop)
        self.upstream = http.server.ThreadingHTTPServer(('127.0.0.1', 0), proxy.Handler)
        self.upstream.pause_tickets = PauseTickets(lambda hold: (hold(), {'ok': False, 'reason': 'delete_unconfirmed'})[1], ttl=0.1)
        self.gateway = http.server.ThreadingHTTPServer(('127.0.0.1', 0), gateway.Handler)
        connect = patch.object(gateway, 'upstream_connection', side_effect=lambda: http.client.HTTPConnection('127.0.0.1', self.upstream.server_address[1]))
        connect.start(); self.addCleanup(connect.stop)
        for server in (self.upstream, self.gateway):
            thread = threading.Thread(target=server.serve_forever, daemon=True); thread.start()
            self.addCleanup(thread.join, 2); self.addCleanup(server.server_close); self.addCleanup(server.shutdown)

    def request(self, path, *, owner=False, origin='https://fixture.run.app', body=None, method='POST'):
        headers = {'Host': 'fixture.run.app', 'Origin': origin}
        if owner: headers['Cookie'] = '__Host-momso-session=fixture-owner'
        data = json.dumps(body or {}).encode() if method == 'POST' else None
        conn = http.client.HTTPConnection('127.0.0.1', self.gateway.server_address[1], timeout=5)
        conn.request(method, path, data, headers)
        response = conn.getresponse()
        raw = response.read()
        result = (response.status, dict(response.getheaders()), json.loads(raw) if raw.startswith(b'{') else {})
        conn.close()
        return result

    def test_no_session_or_wrong_origin_is_rejected(self):
        status, _, value = self.request('/__momso/pause/prepare')
        self.assertEqual((status, value['error']), (401, 'pause_requires_app_session'))
        self.assertEqual(self.request('/__momso/pause/confirm', origin='https://evil.example', body={'ticket': 'x'})[0], 403)
        self.assertEqual(self.request('/__momso/pause/status', method='GET', owner=True, origin='https://evil.example')[0], 403)

    def test_preflight_allows_only_configured_origin_and_content_type(self):
        for origin, expected in [('https://login.example.test', 204), ('https://evil.example', 403)]:
            conn = http.client.HTTPConnection('127.0.0.1', self.gateway.server_address[1], timeout=5)
            conn.request('OPTIONS', '/__momso/pause/confirm', headers={'Host': 'fixture.run.app', 'Origin': origin,
                         'Access-Control-Request-Method': 'POST', 'Access-Control-Request-Headers': 'content-type'})
            response = conn.getresponse()
            self.assertEqual(response.status, expected)
            if expected == 204:
                self.assertEqual(response.getheader('Access-Control-Allow-Origin'), origin)
                self.assertIsNone(response.getheader('Access-Control-Allow-Credentials'))
                self.assertEqual(response.getheader('Access-Control-Expose-Headers'), 'X-Momso-Response')
            response.read(); conn.close()

    def test_ticket_is_one_use_and_release_is_cors_limited(self):
        status, _, prepared = self.request('/__momso/pause/prepare', owner=True)
        self.assertEqual(status, 200)
        path = '/__momso/pause/confirm'
        status, headers, confirmed = self.request(path, origin='https://login.example.test', body={'ticket': prepared['ticket']})
        self.assertEqual(status, 200)
        self.assertEqual(headers['Access-Control-Allow-Origin'], 'https://login.example.test')
        self.assertNotIn('Access-Control-Allow-Credentials', headers)
        self.assertEqual(self.request(path, origin='https://login.example.test', body={'ticket': prepared['ticket']})[0], 409)
        self.assertEqual(self.request('/__momso/pause/release', origin='https://login.example.test', body={'ticket': confirmed['releaseTicket']})[0], 200)

    def test_expired_ticket_cannot_confirm(self):
        _, _, prepared = self.request('/__momso/pause/prepare', owner=True)
        time.sleep(0.2)
        self.assertEqual(self.request('/__momso/pause/confirm', origin='https://login.example.test', body={'ticket': prepared['ticket']})[0], 409)

    def test_backup_failure_does_not_issue_ticket(self):
        self.upstream.pause_tickets = PauseTickets(lambda _: {'ok': False, 'reason': 'backup_not_connected'})
        status, _, result = self.request('/__momso/pause/prepare', owner=True)
        self.assertEqual((status, result.get('reason')), (409, 'backup_not_connected'))
        self.assertNotIn('ticket', result)


if __name__ == '__main__':
    unittest.main()
