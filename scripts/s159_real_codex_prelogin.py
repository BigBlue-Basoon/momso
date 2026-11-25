"""Exercise an isolated server and installed Codex without completing login."""
import json
import os
from pathlib import Path
import pty
import select
import socket
import subprocess
import sys
import tempfile
import time
import urllib.request


def call(port, path, method='GET', session=None):
    headers = {'Authorization': 'Bearer s159-test-only'}
    if session:
        headers['x-momso-login-session'] = session
    request = urllib.request.Request(f'http://127.0.0.1:{port}{path}', headers=headers,
                                     method=method, data=b'' if method == 'POST' else None)
    with urllib.request.urlopen(request, timeout=10) as response:
        return json.load(response)


def main():
    executable, codex = map(str, sys.argv[1:])
    with tempfile.TemporaryDirectory(prefix='momso-s159-prelogin-') as temp:
        root = Path(temp)
        master, slave = pty.openpty()
        env = {'HOME': str(root), 'PATH': '/usr/bin:/bin', 'TERM': 'xterm-256color',
               'LANG': 'C.UTF-8', 'NO_COLOR': '1', 'BROWSER': '/bin/false'}
        interactive = subprocess.run([codex], cwd=root, env=env, stdin=slave,
                                     stdout=slave, stderr=slave, timeout=15)
        os.close(slave)
        output = bytearray()
        while select.select([master], [], [], .1)[0]:
            try:
                output.extend(os.read(master, 8192))
            except OSError:
                break
        os.close(master)
        print('interactive exit:', interactive.returncode)
        print('interactive error:', output.decode('utf-8', 'replace').replace(temp, '<isolated-home>').strip())
        with socket.socket() as sock:
            sock.bind(('127.0.0.1', 0))
            port = sock.getsockname()[1]
        env.update({'MOMSO_SERVER_TOKEN': 's159-test-only', 'MOMSO_SERVER_DATA_DIR': str(root / 'data'),
                    'MOMSO_SERVER_LISTEN': f'127.0.0.1:{port}', 'MOMSO_CODEX_PATH': codex,
                    'MOMSO_TEST_DISABLE_AUTO_BACKUP': '1'})
        server = subprocess.Popen([executable], env=env, stdout=subprocess.DEVNULL, stderr=subprocess.DEVNULL)
        try:
            for _ in range(100):
                try:
                    if call(port, '/health')['state'] == 'ready':
                        break
                except (OSError, ValueError):
                    time.sleep(.1)
            else:
                raise RuntimeError('server did not become ready')
            before = call(port, '/providers')
            print('server provider before:', next(x['state'] for x in before['providers'] if x['provider'] == 'codex'))
            session = call(port, '/providers/codex/login', 'POST')['sessionId']
            for _ in range(120):
                state = call(port, '/providers/codex/login', session=session)['state']
                if state != 'starting':
                    break
                time.sleep(.5)
            print('server device login state:', state)
            call(port, '/providers/codex/login/cancel', 'POST', session)
            if state != 'waiting':
                raise RuntimeError('device login did not reach waiting')
            codex_env = dict(env, CODEX_HOME=str(root / 'data' / 'codex'))
            for name in ('MOMSO_SERVER_TOKEN', 'CREDENTIALS_DIRECTORY', 'ANTHROPIC_API_KEY', 'GEMINI_API_KEY'):
                codex_env.pop(name, None)
            cli = subprocess.Popen([codex, 'app-server'], env=codex_env, cwd=root / 'data' / 'workspace',
                                   stdin=subprocess.PIPE, stdout=subprocess.PIPE, stderr=subprocess.DEVNULL,
                                   text=True, bufsize=1)
            try:
                def send(value):
                    cli.stdin.write(json.dumps(value) + '\n')
                    cli.stdin.flush()

                def receive(request_id):
                    deadline = time.monotonic() + 30
                    while time.monotonic() < deadline:
                        if select.select([cli.stdout], [], [], 1)[0]:
                            line = cli.stdout.readline()
                            if not line:
                                break
                            message = json.loads(line)
                            if message.get('id') == request_id:
                                return message
                    raise RuntimeError('app-server response unavailable')

                send({'id': 1, 'method': 'initialize', 'params': {'clientInfo': {'name': 'momso', 'title': 'momso', 'version': '0.1.0'}, 'capabilities': {'experimentalApi': True}}})
                first = receive(1)
                if 'error' in first:
                    raise RuntimeError('app-server initialize failed')
                send({'method': 'initialized', 'params': {}})
                send({'id': 2, 'method': 'thread/start', 'params': {'cwd': str(root / 'data' / 'workspace'), 'approvalPolicy': 'on-request', 'sandbox': 'workspace-write'}})
                thread = receive(2)
                print('app-server initialize: ok; thread/start:', 'error' if 'error' in thread else 'ok')
                if 'error' in thread:
                    print('app-server thread error:', json.dumps(thread['error'], ensure_ascii=False))
                    raise RuntimeError('app-server thread preparation failed')
            finally:
                cli.terminate()
                cli.wait(timeout=15)
        finally:
            server.terminate()
            server.wait(timeout=15)


if __name__ == '__main__':
    main()
