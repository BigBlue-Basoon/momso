"""Shared verification for update and desktop installation release manifests."""
import json
import re
import os
import shutil
import subprocess
import tempfile
from pathlib import Path
from urllib.parse import urlparse


class ManifestError(Exception):
    pass


def _openssl():
    executable = shutil.which('openssl')
    if not executable and os.name == 'nt':
        git = shutil.which('git')
        if git:
            git_path = Path(git).resolve()
            for root in (git_path.parent.parent, git_path.parent.parent.parent):
                candidate = root / 'usr/bin/openssl.exe'
                if candidate.is_file():
                    executable = str(candidate)
                    break
    if not executable: raise ManifestError('openssl_unavailable')
    return executable


def verify(manifest, signature, public_key, *, allow_http=False, channel='production'):
    if channel not in ('production', 'development'):
        raise ManifestError('channel_invalid')
    if len(manifest) > 65536 or len(signature) != 64:
        raise ManifestError('signature_invalid')
    openssl = _openssl()
    flags = getattr(subprocess, 'CREATE_NO_WINDOW', 0) if os.name == 'nt' else 0
    key = subprocess.run([openssl, 'pkey', '-pubin', '-in', str(public_key), '-outform', 'DER'], stdout=subprocess.PIPE, stderr=subprocess.DEVNULL, check=False, creationflags=flags)
    if key.returncode or len(key.stdout) != 44 or key.stdout[:12] != bytes.fromhex('302a300506032b6570032100'):
        raise ManifestError('key_invalid')
    with tempfile.TemporaryDirectory(prefix='momso-manifest-') as directory:
        path = Path(directory)
        (path / 'manifest').write_bytes(manifest)
        (path / 'signature').write_bytes(signature)
        result = subprocess.run([openssl, 'pkeyutl', '-verify', '-pubin', '-inkey', str(public_key), '-rawin', '-in', str(path / 'manifest'), '-sigfile', str(path / 'signature')], stdout=subprocess.DEVNULL, stderr=subprocess.DEVNULL, check=False, creationflags=flags)
        if result.returncode:
            raise ManifestError('signature_invalid')
    try:
        def invalid_constant(_value):
            raise ValueError('invalid JSON constant')
        data = json.loads(manifest.decode('utf-8-sig'), parse_constant=invalid_constant)
        url = urlparse(data['bundleUrl'])
        if (data.get('channel', 'production') != channel
                or type(data['sequence']) is not int or not 1 <= data['sequence'] <= 9007199254740991
                or not re.fullmatch(r'[0-9]+\.[0-9]+\.[0-9]+', data['version'])
                or not re.fullmatch(r'[a-f0-9]{64}', data['bundleSha256'])
                or not re.fullmatch(r'[a-zA-Z0-9._/-]+@sha256:[a-f0-9]{64}', data['gatewayImage'])
                or url.scheme not in ({'https', 'http'} if allow_http else {'https'})
                or not url.hostname or url.username or url.password or url.fragment):
            raise ValueError()
    except (KeyError, TypeError, ValueError, json.JSONDecodeError):
        raise ManifestError('manifest_invalid') from None
    return data
