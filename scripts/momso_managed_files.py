"""Atomically replace installer-owned VM files and retain a rollback journal."""
import base64
import hashlib
import json
import os
from pathlib import Path
import re
import tempfile


def _hash(value):
    return hashlib.sha256(value).hexdigest()


def _trusted_dir(path):
    if path.is_symlink() or not path.is_dir():
        raise RuntimeError('managed_directory_conflict')
    stat = path.stat()
    if stat.st_uid != os.geteuid() or stat.st_mode & 0o022:
        raise RuntimeError('managed_directory_conflict')


def _read(path):
    if not os.path.lexists(path):
        return None
    if path.is_symlink() or not path.is_file():
        name = path.name if re.fullmatch(r'[a-z0-9_.-]{1,80}', path.name) else 'unknown'
        raise RuntimeError('momso-managed-conflict: ' + name + ': not_regular')
    return path.read_bytes()


def _write(path, data, mode):
    fd, temporary = tempfile.mkstemp(prefix='.momso-managed-', dir=path.parent)
    try:
        with os.fdopen(fd, 'wb') as stream:
            stream.write(data)
            stream.flush()
            os.fsync(stream.fileno())
        os.chmod(temporary, mode)
        os.replace(temporary, path)
        directory = os.open(path.parent, os.O_RDONLY | os.O_DIRECTORY)
        try:
            os.fsync(directory)
        finally:
            os.close(directory)
    finally:
        if os.path.exists(temporary):
            os.unlink(temporary)


def conflicts(root, manifest, owner, files, legacy, *, adopt=(), first_migration=False):
    """Inspect a group without writing; only fixed generated names may be adopted."""
    root, manifest = map(Path, (root, manifest))
    allowed = {'idle-token.py', 'momso-vault-credential.service', 'momso-idle-stop.service', 'vault.conf', 'backup.conf'}
    adopt = set(adopt)
    if not adopt <= allowed:
        raise RuntimeError('managed_adoption_conflict')
    for directory in (root if os.path.lexists(root) else root.parent, manifest.parent):
        try:
            _trusted_dir(directory)
        except RuntimeError:
            name = directory.name if re.fullmatch(r'[a-z0-9_.-]{1,80}', directory.name) else 'unknown'
            return [(name, 'directory_untrusted')]
    results = []
    original = None
    present = os.path.lexists(manifest)
    try:
        original = _read(manifest)
    except RuntimeError:
        results.append((manifest.name, 'not_regular'))
    recorded = {}
    if original is not None:
        try:
            value = json.loads(original)
            if not isinstance(value, dict) or value.get('version') != 1 or value.get('owner') != owner or not isinstance(value.get('files'), dict):
                raise ValueError('invalid manifest')
            recorded = value['files']
        except (ValueError, UnicodeDecodeError):
            results.append((manifest.name, 'manifest_invalid'))
    for name, data in files.items():
        if not re.fullmatch(r'[a-z0-9_.-]{1,80}', name) or Path(name).name != name or not isinstance(data, bytes):
            results.append(('unknown', 'invalid_name'))
            continue
        path = root / name
        if not os.path.lexists(path):
            continue
        if path.is_symlink() or not path.is_file():
            results.append((name, 'not_regular'))
            continue
        old = path.read_bytes()
        if _hash(old) not in {recorded.get(name), _hash(data), *legacy.get(name, ())}:
            if not (first_migration and not present and name in adopt):
                results.append((name, 'unrecognized'))
    return results


def rollback(root, manifest, journal, owner):
    root, manifest, journal = map(Path, (root, manifest, journal))
    raw = _read(journal)
    if raw is None:
        return
    for directory in (root, manifest.parent, journal.parent):
        _trusted_dir(directory)
    record = json.loads(raw)
    if record.get('owner') != owner or record.get('version') != 1:
        raise RuntimeError('managed_journal_conflict')
    current_manifest = _read(manifest)
    old_manifest = record['manifest']
    old_manifest_bytes = None if old_manifest is None else base64.b64decode(old_manifest, validate=True)
    if current_manifest is not None and _hash(current_manifest) not in (record['new_manifest_hash'], _hash(old_manifest_bytes) if old_manifest_bytes is not None else ''):
        raise RuntimeError('managed_rollback_conflict')
    entries = []
    for name, item in record['files'].items():
        if Path(name).name != name:
            raise RuntimeError('managed_journal_conflict')
        path = root / name
        current = _read(path)
        old = None if item['old'] is None else base64.b64decode(item['old'], validate=True)
        if current is not None and _hash(current) not in (item['new_hash'], _hash(old) if old is not None else ''):
            raise RuntimeError('managed_rollback_conflict')
        entries.append((path, current, old, item['mode']))
    for path, current, old, mode in entries:
        if old is None:
            if current is not None:
                path.unlink()
        else:
            _write(path, old, mode)
    if old_manifest is None:
        if os.path.lexists(manifest):
            _read(manifest)
            manifest.unlink()
    else:
        _write(manifest, old_manifest_bytes, 0o600)
    journal.unlink()


def install(root, manifest, journal, owner, files, legacy, mode=0o755, *, adopt=(), first_migration=False):
    root, manifest, journal = map(Path, (root, manifest, journal))
    for directory in (root, manifest.parent, journal.parent):
        _trusted_dir(directory)
    if _read(journal) is not None:
        rollback(root, manifest, journal, owner)
    issues = conflicts(root, manifest, owner, files, legacy, adopt=adopt, first_migration=first_migration)
    if issues:
        raise RuntimeError('\n'.join('momso-managed-conflict: ' + name + ': ' + reason for name, reason in issues))
    original_manifest = _read(manifest)
    recorded = {}
    if original_manifest is not None:
        value = json.loads(original_manifest)
        if value.get('version') != 1 or value.get('owner') != owner or not isinstance(value.get('files'), dict):
            raise RuntimeError('managed_manifest_conflict')
        recorded = value['files']
    previous = {}
    for name, data in files.items():
        if not re.fullmatch(r'[a-z0-9_.-]{1,80}', name) or Path(name).name != name or not isinstance(data, bytes):
            raise RuntimeError('momso-managed-conflict: unknown: invalid_name')
        path = root / name
        old = _read(path)
        previous[name] = {'old': None if old is None else base64.b64encode(old).decode('ascii'),
                          'mode': path.stat().st_mode & 0o777 if old is not None else 0o755,
                          'new_hash': _hash(data)}
    hashes = dict(recorded)
    hashes.update({name: _hash(data) for name, data in files.items()})
    updated_manifest = json.dumps({'version': 1, 'owner': owner, 'files': hashes}, sort_keys=True).encode()
    record = {'version': 1, 'owner': owner, 'files': previous, 'new_manifest_hash': _hash(updated_manifest),
              'manifest': None if original_manifest is None else base64.b64encode(original_manifest).decode('ascii')}
    _write(journal, json.dumps(record, sort_keys=True).encode(), 0o600)
    try:
        for name, data in files.items():
            _write(root / name, data, mode)
        _write(manifest, updated_manifest, 0o600)
    except BaseException:
        rollback(root, manifest, journal, owner)
        raise
