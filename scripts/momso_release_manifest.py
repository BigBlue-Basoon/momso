"""Verify signed release input before any cloud-side installation changes."""
import hashlib
import importlib.util
import tarfile
import tempfile
import urllib.request
from pathlib import Path, PurePosixPath
from urllib.parse import urlsplit, urlunsplit


_path = Path(__file__).resolve().parent.parent / 'apps/alpha/desktop/src-tauri/momso-server-update/release_manifest.py'
_spec = importlib.util.spec_from_file_location('momso_release_manifest_shared', _path)
_shared = importlib.util.module_from_spec(_spec)
_spec.loader.exec_module(_shared)
ManifestError = _shared.ManifestError
verify = _shared.verify


def download(url, limit, *, allow_http=False):
    parsed = urlsplit(url)
    if parsed.scheme not in ({'https', 'http'} if allow_http else {'https'}) or not parsed.hostname or parsed.username or parsed.password or parsed.fragment:
        raise ManifestError('invalid_url')
    with urllib.request.urlopen(url, timeout=30) as response:
        final = urlsplit(response.geturl())
        if final.scheme not in ({'https', 'http'} if allow_http else {'https'}) or not final.hostname or final.username or final.password:
            raise ManifestError('invalid_url')
        data = response.read(limit + 1)
    if len(data) > limit:
        raise ManifestError('download_too_large')
    return data


def signature_url(url):
    parsed = urlsplit(url)
    return urlunsplit((parsed.scheme, parsed.netloc, parsed.path + '.sig', parsed.query, ''))


def release(url, public_key, *, allow_http=False, channel='production'):
    return verify(download(url, 65536, allow_http=allow_http), download(signature_url(url), 1024, allow_http=allow_http), public_key, allow_http=allow_http, channel=channel)


def _members(archive, prefix=''):
    result = {}
    total = 0
    for item in archive:
        parts = PurePosixPath(item.name).parts
        if not parts and item.isdir():
            continue
        if not parts or item.name.startswith('/') or '..' in parts or not (item.isfile() or item.isdir()):
            raise ManifestError('invalid_bundle')
        name = '/'.join(parts)
        if item.isdir():
            continue
        total += item.size
        if total > 2147483648 or name in result or len(result) >= 100000:
            raise ManifestError('invalid_bundle')
        if prefix and not name.startswith(prefix):
            continue
        stream = archive.extractfile(item)
        if stream is None:
            raise ManifestError('invalid_bundle')
        digest = hashlib.sha256()
        for chunk in iter(lambda: stream.read(1024 * 1024), b''):
            digest.update(chunk)
        result[name] = digest.hexdigest()
    return result


def verify_inputs(bundle, expected_digest, server, web):
    if hashlib.sha256(bundle).hexdigest() != expected_digest:
        raise ManifestError('digest_mismatch')
    try:
        with tempfile.TemporaryFile() as file:
            file.write(bundle)
            file.seek(0)
            with tarfile.open(fileobj=file, mode='r:gz') as archive:
                files = _members(archive)
        with tarfile.open(web, mode='r:gz') as archive:
            web_files = _members(archive)
        server_hash = hashlib.sha256()
        with Path(server).open('rb') as stream:
            for chunk in iter(lambda: stream.read(1024 * 1024), b''):
                server_hash.update(chunk)
        expected_web = {name[4:]: digest for name, digest in files.items() if name.startswith('web/')}
        if files.get('momso-server') != server_hash.hexdigest() or not expected_web.get('index.html') or web_files != expected_web:
            raise ManifestError('artifact_mismatch')
    except (tarfile.TarError, OSError) as error:
        raise ManifestError('invalid_bundle') from error
