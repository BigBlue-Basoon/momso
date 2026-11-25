"""Measure a fresh, unauthenticated CLI in an isolated pseudo-terminal."""
import fcntl
import os
import pty
import re
import select
import signal
import struct
import subprocess
import sys
import tempfile
import termios
import time
from pathlib import Path


def process_tree(root):
    parents = {}
    for path in Path('/proc').iterdir():
        if not path.name.isdecimal():
            continue
        try:
            status = (path / 'status').read_text()
            parent = next(int(line.split()[1]) for line in status.splitlines() if line.startswith('PPid:'))
            parents[int(path.name)] = parent
        except (OSError, StopIteration, ValueError):
            continue
    ids = {root}
    while True:
        more = {pid for pid, parent in parents.items() if parent in ids}
        if more <= ids:
            return ids
        ids |= more


def sample(root):
    ids = process_tree(root)
    total = 0
    seen = 0
    for pid in ids:
        try:
            rollup = Path(f'/proc/{pid}/smaps_rollup').read_text()
            total += next(int(line.split()[1]) for line in rollup.splitlines() if line.startswith('Pss:'))
            seen += 1
        except (OSError, StopIteration, ValueError):
            pass
    return total, seen, len(ids)


def main():
    if len(sys.argv) < 2:
        raise SystemExit('an executable path is required')
    with tempfile.TemporaryDirectory(prefix='momso-s159-cli-') as home:
        master, slave = pty.openpty()
        fcntl.ioctl(slave, termios.TIOCSWINSZ, struct.pack('HHHH', 24, 80, 0, 0))
        fcntl.fcntl(master, fcntl.F_SETFL, os.O_NONBLOCK)
        env = {'HOME': home, 'PATH': os.environ['PATH'], 'TERM': 'xterm-256color', 'LANG': 'C.UTF-8', 'NO_COLOR': '1', 'BROWSER': '/bin/false'}
        child = subprocess.Popen(sys.argv[1:], stdin=slave, stdout=slave, stderr=slave, cwd=home, env=env, start_new_session=True)
        os.close(slave)
        output = bytearray()
        last = None
        advanced = set()
        start = time.monotonic()
        try:
            while time.monotonic() - start < 15 and child.poll() is None:
                if select.select([master], [], [], 0.2)[0]:
                    try:
                        output.extend(os.read(master, 8192))
                    except OSError:
                        pass
                visible = re.sub(rb'\x1b\[[0-?]*[ -/]*[@-~]', b'', bytes(output).lower())
                visible = re.sub(rb'[^a-z]', b'', visible)
                for marker in (b'choosethetextstyle', b'trustingafolderallows'):
                    if marker in visible and marker not in advanced:
                        os.write(master, b'\r')
                        advanced.add(marker)
                if time.monotonic() - start >= 0.2:
                    measured = sample(child.pid)
                    if measured[1]:
                        last = measured
            state = 'waiting' if child.poll() is None else f'exited_{child.returncode}'
            try:
                while True:
                    chunk = os.read(master, 8192)
                    if not chunk:
                        break
                    output.extend(chunk)
            except OSError:
                pass
            login = any(word in output.lower() for word in (b'login', b'log in', b'sign in', b'authenticate', b'authentication'))
            print(f'CLI state={state} login_hint={login} pss_kib={last[0] if last else "unavailable"} pss_read={last[1] if last else 0} processes={last[2] if last else 0}', flush=True)
            text = re.sub(r'\x1b\[[0-?]*[ -/]*[@-~]', '', output.decode('utf-8', 'replace'))
            text = text.replace(home, '<isolated-home>')
            text = re.sub(r'https?://[^\s]+', '<url>', text)
            text = re.sub(r'\b[A-Z0-9]{4}-[A-Z0-9]{4}\b', '<code>', text)
            print('CLI screen text:', repr(text[-1000:]), flush=True)
        finally:
            if child.poll() is None:
                os.killpg(child.pid, signal.SIGTERM)
                try:
                    child.wait(timeout=5)
                except subprocess.TimeoutExpired:
                    os.killpg(child.pid, signal.SIGKILL)
                    child.wait()
            os.close(master)


if __name__ == '__main__':
    main()
