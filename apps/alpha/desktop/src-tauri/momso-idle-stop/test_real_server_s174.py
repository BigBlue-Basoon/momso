"""Exercise the real server process with a local-only backup and fake stop."""
import datetime as dt
import importlib.util
import json
import os
from pathlib import Path
import socket
import subprocess
import tempfile
import time
import unittest


@unittest.skipUnless(os.environ.get('MOMSO_S174_SERVER') and os.environ.get('MOMSO_S174_IDLE'), 'requires isolated server fixture')
class RealIdleServer(unittest.TestCase):
    def test_real_server_gate_backup_and_stop(self):
        spec = importlib.util.spec_from_file_location('fixture_idle', os.environ['MOMSO_S174_IDLE'])
        idle = importlib.util.module_from_spec(spec); spec.loader.exec_module(idle)
        with tempfile.TemporaryDirectory() as tmp:
            root = Path(tmp); token = root / 'token'; token.write_text('fixture-token')
            sock = socket.socket(); sock.bind(('127.0.0.1', 0)); port = sock.getsockname()[1]; sock.close()
            env = {**os.environ, 'MOMSO_SERVER_TOKEN': 'fixture-token', 'MOMSO_SERVER_DATA_DIR': str(root / 'data'),
                   'MOMSO_SERVER_LISTEN': f'127.0.0.1:{port}', 'MOMSO_CODEX_PATH': '/nonexistent',
                   'MOMSO_BACKUP_AGENT': os.environ['MOMSO_S174_AGENT']}
            process = subprocess.Popen([os.environ['MOMSO_S174_SERVER']], env=env, stdout=subprocess.DEVNULL, stderr=subprocess.DEVNULL)
            try:
                server = idle.Server(token, port); deadline = time.monotonic() + 30
                while True:
                    try:
                        view = server.call('/activity')
                        if view.get('shutdownReady') is True: break
                    except OSError: pass
                    if time.monotonic() >= deadline: self.fail('initial_backup_not_ready')
                    time.sleep(.2)
                # Use the real last-activity timestamp and wait the full threshold.
                last = dt.datetime.fromisoformat(view['lastActivityAt'].replace('Z', '+00:00'))
                remaining = 60 - (dt.datetime.now(dt.timezone.utc) - last).total_seconds()
                if remaining > 0: time.sleep(remaining + .2)
                trace = []
                original = server.call
                def call(path, method='GET'):
                    try:
                        result = original(path, method)
                        trace.append((path, {k: result.get(k) for k in ('maintenance', 'quiescent', 'shutdownReady', 'activeBackups', 'busy', 'backupStatus')}))
                        return result
                    except Exception as error:
                        trace.append((path, type(error).__name__, getattr(error, 'code', None)))
                        raise
                server.call = call
                class Compute:
                    called = False
                    def stop_self(self): self.called = True; return True
                compute = Compute(); monitor = idle.IdleStop(server, compute, root / 'state', 60)
                try:
                    result = monitor.step()
                except Exception as error:
                    print('fixture_error:', type(error).__name__, str(error), flush=True)
                    print('fixture_trace:', json.dumps(trace), flush=True)
                    if os.environ.get('MOMSO_S174_EXPECT_OLD') == '1': return
                    raise
                print('fixture_result:', result, 'fixture_trace:', json.dumps(trace), flush=True)
                if os.environ.get('MOMSO_S174_EXPECT_OLD') == '1':
                    self.assertFalse(compute.called)
                else:
                    self.assertEqual(result, 'stop_requested'); self.assertTrue(compute.called)
            finally:
                process.terminate()
                try: process.wait(timeout=5)
                except subprocess.TimeoutExpired: process.kill(); process.wait()


if __name__ == '__main__': unittest.main()
