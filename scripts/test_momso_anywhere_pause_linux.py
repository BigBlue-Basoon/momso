"""Exercise the pause adapter with the updater's actual Linux file lock."""
import http.server
import json
from pathlib import Path
import subprocess
import sys
import tempfile
import threading
import time
import unittest

from momso_anywhere_pause_guard import pause_using_server_admission

if sys.platform == 'linux':
    sys.path.insert(0, str(Path(__file__).resolve().parents[1] / 'apps/alpha/desktop/src-tauri/momso-server-update'))
    from server_admission import ServerAdmission


class Handler(http.server.BaseHTTPRequestHandler):
    def log_message(self, *_args): pass
    def reply(self, value):
        body = json.dumps(value).encode()
        self.send_response(200)
        self.send_header('Content-Length', str(len(body)))
        self.end_headers()
        self.wfile.write(body)
    def do_GET(self):
        assert self.headers['Authorization'] == 'Bearer fixture-token'
        self.reply({'maintenance': self.server.gated, 'quiescent': self.server.gated,
                    'busy': False, 'maintenanceReady': self.server.gated,
                    'githubBackup': {'state': 'saved', 'lastSuccessAt': 124},
                    'objectBackup': {'state': 'saved', 'lastSuccessAt': 124}})
    def do_POST(self):
        assert self.headers['Authorization'] == 'Bearer fixture-token'
        self.server.gated = self.path == '/maintenance'
        self.reply({'maintenance': self.server.gated, 'closedAt': 123})


@unittest.skipUnless(sys.platform == 'linux', 'uses the Linux lifecycle lock')
class LocalAdmissionTest(unittest.TestCase):
    def test_confirmed_lease_survives_process_restart(self):
        from momso_anywhere_vm_pause import create_vm_pause
        with tempfile.TemporaryDirectory() as folder:
            root = Path(folder)
            token = root / 'token'
            token.write_text('fixture-token', encoding='utf-8')
            updater = Path(__file__).resolve().parents[1] / 'apps/alpha/desktop/src-tauri/momso-server-update'
            server = http.server.ThreadingHTTPServer(('127.0.0.1', 0), Handler)
            server.gated = False
            thread = threading.Thread(target=server.serve_forever, daemon=True)
            thread.start()
            code = '''import sys,time\nfrom momso_anywhere_vm_pause import create_vm_pause\np=create_vm_pause(token=sys.argv[1], lifecycle=sys.argv[2], updater=sys.argv[3], port=int(sys.argv[4]))\nr=p.prepare(); assert r['ok'],r\nassert p.confirm(r['ticket'])['ok']\nprint('confirmed',flush=True)\nwhile True: time.sleep(1)\n'''
            child = subprocess.Popen([sys.executable, '-c', code, str(token), str(root / 'lifecycle'), str(updater), str(server.server_port)],
                                     cwd=Path(__file__).resolve().parent, stdout=subprocess.PIPE, stderr=subprocess.PIPE, text=True)
            try:
                self.assertEqual(child.stdout.readline().strip(), 'confirmed')
                self.assertTrue(server.gated)
                child.kill()
                child.wait(timeout=5)
                tickets = create_vm_pause(token=token, lifecycle=root / 'lifecycle', updater=updater, port=server.server_port)
                deadline = time.monotonic() + 5
                while tickets.status()['state'] == 'recovering' and time.monotonic() < deadline:
                    time.sleep(0.01)
                self.assertEqual(tickets.status()['state'], 'confirmed')
                self.assertTrue(server.gated)
                competing = ServerAdmission(token, root / 'lifecycle', port=server.server_port, timeout=1)
                self.assertFalse(competing.acquire())
                self.assertTrue(tickets.release(owner=True)['ok'])
                self.assertFalse(server.gated)
                self.assertFalse((root / 'lifecycle' / 'pause-ticket.pending').exists())
            finally:
                if child.poll() is None:
                    child.kill()
                    child.wait(timeout=5)
                child.stdout.close()
                child.stderr.close()
                server.shutdown()
                server.server_close()
                thread.join()

    def test_gate_and_real_file_lock_remain_held_during_delete(self):
        with tempfile.TemporaryDirectory() as folder:
            root = Path(folder)
            token = root / 'token'
            token.write_text('fixture-token', encoding='utf-8')
            server = http.server.ThreadingHTTPServer(('127.0.0.1', 0), Handler)
            server.gated = False
            thread = threading.Thread(target=server.serve_forever, daemon=True)
            thread.start()
            admission = ServerAdmission(token, root / 'lifecycle', port=server.server_port, timeout=1)
            competing = ServerAdmission(token, root / 'lifecycle', port=server.server_port, timeout=1)
            try:
                def delete():
                    self.assertTrue(server.gated)
                    self.assertFalse(competing.acquire())
                    return True
                self.assertTrue(pause_using_server_admission(admission, delete, sleep=lambda _: None)['ok'])
            finally:
                if admission.held: admission.release()
                server.shutdown()
                server.server_close()
                thread.join()


if __name__ == '__main__': unittest.main()
