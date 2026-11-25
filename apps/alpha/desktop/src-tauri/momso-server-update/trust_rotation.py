"""Threshold-verified root rotation and persistent trust state."""
import base64
import binascii
import datetime
import importlib.util
import json
import os
import re
import subprocess
import tempfile
from pathlib import Path

class TrustError(Exception):
    pass


PREFIX = bytes.fromhex('302a300506032b6570032100')
MAX_BYTES = 65536


def _unique_object(pairs):
    result = {}
    for key, value in pairs:
        if key in result:
            raise TrustError('root_invalid')
        result[key] = value
    return result


def _key(value):
    try:
        raw = base64.b64decode(value, validate=True)
    except (TypeError, ValueError, binascii.Error):
        raise TrustError('root_invalid') from None
    if len(raw) != 44 or not raw.startswith(PREFIX):
        raise TrustError('root_invalid')
    if base64.b64encode(raw).decode('ascii') != value:
        raise TrustError('root_invalid')
    return raw


def parse_root(raw):
    if not isinstance(raw, bytes) or len(raw) > MAX_BYTES:
        raise TrustError('root_invalid')
    try:
        root = json.loads(raw.decode('utf-8'), object_pairs_hook=_unique_object,
                          parse_constant=lambda _: (_ for _ in ()).throw(ValueError()))
        required = {'version', 'expires', 'threshold', 'keys', 'releaseKey'}
        if set(root) not in (required, required | {'issuedAt'}):
            raise ValueError()
        if type(root['version']) is not int or not 0 <= root['version'] <= 9007199254740991:
            raise ValueError()
        if not isinstance(root['keys'], list) or len(root['keys']) != 3 or root['threshold'] != 2 or type(root['threshold']) is not int:
            raise ValueError()
        keys = [_key(value) for value in root['keys']]
        if len(set(keys)) != 3 or _key(root['releaseKey']) not in keys:
            raise ValueError()
        if not isinstance(root['expires'], str) or not re.fullmatch(r'\d{4}-\d{2}-\d{2}T\d{2}:\d{2}:\d{2}Z', root['expires']):
            raise ValueError()
        datetime.datetime.strptime(root['expires'], '%Y-%m-%dT%H:%M:%SZ')
        if root['version'] > 0 and 'issuedAt' not in root:
            raise ValueError()
        if 'issuedAt' in root:
            _timestamp(root['issuedAt'])
        return root
    except (ValueError, TypeError, KeyError, UnicodeError, AttributeError):
        raise TrustError('root_invalid') from None


def _verify(raw, signature, key):
    spec = importlib.util.spec_from_file_location('momso_rotation_release_manifest', Path(__file__).with_name('release_manifest.py'))
    manifest = importlib.util.module_from_spec(spec); spec.loader.exec_module(manifest)
    if not isinstance(signature, bytes) or len(signature) != 64:
        return False
    with tempfile.TemporaryDirectory(prefix='momso-root-') as directory:
        path = Path(directory)
        (path / 'root').write_bytes(raw)
        (path / 'signature').write_bytes(signature)
        (path / 'key').write_bytes(key)
        flags = getattr(subprocess, 'CREATE_NO_WINDOW', 0) if os.name == 'nt' else 0
        result = subprocess.run([manifest._openssl(), 'pkeyutl', '-verify', '-pubin', '-keyform', 'DER', '-inkey', str(path / 'key'), '-rawin', '-in', str(path / 'root'), '-sigfile', str(path / 'signature')], stdin=subprocess.DEVNULL, stdout=subprocess.DEVNULL, stderr=subprocess.DEVNULL, check=False, timeout=30, creationflags=flags)
        return result.returncode == 0


def _timestamp(value):
    if not isinstance(value, str) or not re.fullmatch(r'\d{4}-\d{2}-\d{2}T\d{2}:\d{2}:\d{2}Z', value):
        raise ValueError('invalid_timestamp')
    return datetime.datetime.strptime(value, '%Y-%m-%dT%H:%M:%SZ').replace(tzinfo=datetime.timezone.utc)


def verify_rotation(current_raw, candidate_raw, signatures, *, now=None):
    current, candidate = parse_root(current_raw), parse_root(candidate_raw)
    if candidate['version'] != current['version'] + 1:
        raise TrustError('root_version_invalid')
    now = now or datetime.datetime.now(datetime.timezone.utc)
    if now.tzinfo is None:
        raise TrustError('clock_invalid')
    issued = _timestamp(candidate['issuedAt'])
    if issued > now or ('issuedAt' in current and issued < _timestamp(current['issuedAt'])):
        raise TrustError('root_time_invalid')
    if issued >= _timestamp(current['expires']) or issued >= _timestamp(candidate['expires']):
        raise TrustError('root_time_invalid')
    if not isinstance(signatures, dict) or len(signatures) > 6:
        raise TrustError('signature_invalid')
    valid = set()
    for encoded in set(current['keys']) | set(candidate['keys']):
        signature = signatures.get(encoded)
        if signature is not None and _verify(candidate_raw, signature, _key(encoded)):
            valid.add(encoded)
    if len(valid.intersection(current['keys'])) < 2 or len(valid.intersection(candidate['keys'])) < 2:
        raise TrustError('signature_threshold_invalid')
    return candidate


def current_release_key(root_raw, *, now=None):
    root = parse_root(root_raw)
    now = now or datetime.datetime.now(datetime.timezone.utc)
    if now.tzinfo is None:
        raise TrustError('clock_invalid')
    if 'issuedAt' in root and _timestamp(root['issuedAt']) > now:
        raise TrustError('root_time_invalid')
    expiry = datetime.datetime.strptime(root['expires'], '%Y-%m-%dT%H:%M:%SZ').replace(tzinfo=datetime.timezone.utc)
    if now >= expiry:
        raise TrustError('root_expired')
    return b'-----BEGIN PUBLIC KEY-----\n' + root['releaseKey'].encode('ascii') + b'\n-----END PUBLIC KEY-----\n'


class RootStore:
    """VM-only durable root state. The caller must protect the directory."""
    def __init__(self, directory, bootstrap):
        self.directory = Path(directory)
        parse_root(bootstrap)
        self.bootstrap = bootstrap

    def _locked(self, action):
        self.directory.mkdir(mode=0o700, parents=True, exist_ok=True)
        with (self.directory / 'root.lock').open('a+b') as lock:
            if os.name == 'nt':
                import msvcrt
                if lock.seek(0, os.SEEK_END) == 0:
                    lock.write(b'\0'); lock.flush()
                lock.seek(0); msvcrt.locking(lock.fileno(), msvcrt.LK_LOCK, 1)
            else:
                import fcntl
                fcntl.flock(lock.fileno(), fcntl.LOCK_EX)
            try:
                return action()
            finally:
                if os.name == 'nt':
                    lock.seek(0); msvcrt.locking(lock.fileno(), msvcrt.LK_UNLCK, 1)
                else:
                    fcntl.flock(lock.fileno(), fcntl.LOCK_UN)

    def _read(self):
        path = self.directory / 'root.json'
        if not path.exists():
            return self.bootstrap
        with path.open('rb') as file:
            raw = file.read(MAX_BYTES + 1)
        parse_root(raw)
        return raw

    def current(self):
        return self._locked(self._read)

    def apply(self, candidate, signatures, *, now=None):
        def update():
            current = self._read()
            root = verify_rotation(current, candidate, signatures, now=now)
            fd, name = tempfile.mkstemp(prefix='.root-', dir=self.directory)
            try:
                with os.fdopen(fd, 'wb') as file:
                    file.write(candidate)
                    file.flush()
                    os.fsync(file.fileno())
                os.replace(name, self.directory / 'root.json')
                if os.name != 'nt':
                    directory_fd = os.open(self.directory, os.O_RDONLY | getattr(os, 'O_DIRECTORY', 0))
                    try:
                        os.fsync(directory_fd)
                    finally:
                        os.close(directory_fd)
            finally:
                if os.path.exists(name):
                    os.unlink(name)
            return root
        return self._locked(update)
