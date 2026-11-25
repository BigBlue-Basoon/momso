"""Restore a retained backup before starting any momso service on a new VM.

The installer must invoke this worker while services are stopped and must not
start them unless it returns success. No backup object is written or deleted.
"""
import argparse
import datetime
import fcntl
import hmac
import json
import os
from pathlib import Path
import pwd
import re
import subprocess
import sys
import tempfile
import urllib.parse

import momso_anywhere_backup as backup
from momso_anywhere_backup_google import CloudKms, CloudStorage, GoogleClient


class RestoreFailure(Exception):
    def __init__(self, detail):
        self.detail = detail
        super().__init__('restore_failed')


class CheckedKeys:
    def __init__(self, delegate): self.delegate = delegate
    def unwrap(self, value):
        try:
            key = self.delegate.unwrap(value)
            if not isinstance(key, bytes) or len(key) != 32:
                raise ValueError('invalid key')
            return key
        except Exception: raise RestoreFailure('key_unavailable') from None


def verify_cloud_ownership(client, bucket, key_resource, installation_id):
    if not re.fullmatch(r'[a-f0-9]{12}(?:[a-f0-9]{12})?', installation_id):
        raise RestoreFailure('ownership_unverified')
    short = installation_id[:12]
    key_pattern = rf'projects/[a-z][a-z0-9-]{{4,28}}[a-z0-9]/locations/asia-northeast3/keyRings/momso-backup-{short}/cryptoKeys/backup'
    if bucket != 'momso-backup-' + short or not re.fullmatch(key_pattern, key_resource):
        raise RestoreFailure('ownership_unverified')
    try:
        bucket_info = json.loads(client.request('https://storage.googleapis.com/storage/v1/b/' + urllib.parse.quote(bucket, safe='')))
    except Exception:
        raise RestoreFailure('backup_unreadable') from None
    try:
        key_info = json.loads(client.request('https://cloudkms.googleapis.com/v1/' + key_resource))
    except Exception:
        raise RestoreFailure('key_unavailable') from None
    try:
        if (bucket_info.get('name') != bucket or bucket_info.get('labels', {}).get('momso-install-id') != installation_id
                or key_info.get('name') != key_resource or key_info.get('labels', {}).get('momso-install-id') != installation_id):
            raise ValueError('unexpected resource')
    except Exception:
        raise RestoreFailure('ownership_unverified') from None


def services_stopped(run=subprocess.run):
    for name in ('momso-server.service', 'momso-update.service', 'momso-idle-stop.service', 'momso-backup.service', 'momso-backup.timer'):
        result = run(['systemctl', 'show', name, '--property=ActiveState', '--value'], stdout=subprocess.PIPE, stderr=subprocess.DEVNULL, check=False, timeout=15)
        if result.returncode != 0 or result.stdout.strip() not in (b'inactive', b'failed'):
            return False
    return True


def protect_first_backup(data, source=None):
    marker = data / '.momso-restore-pending-first-backup'
    contents = b'momso-restore-v1\n' if source is None else (f'momso-restore-v2:{source}\n').encode('ascii')
    if source is not None and not re.fullmatch(r'slot-[0-7]@[0-9]{1,32}', source):
        raise RestoreFailure('backup_unreadable')
    if marker.is_symlink():
        raise RestoreFailure('unsafe_data_directory')
    fd, temporary = tempfile.mkstemp(prefix='.momso-restore-marker-', dir=data)
    try:
        with os.fdopen(fd, 'wb') as output:
            output.write(contents)
            output.flush()
            os.fsync(output.fileno())
        try:
            os.link(temporary, marker, follow_symlinks=False)
        except FileExistsError:
            if marker.is_symlink() or not marker.is_file() or marker.read_bytes() != contents:
                raise RestoreFailure('unsafe_data_directory') from None
        directory = os.open(data, os.O_RDONLY | getattr(os, 'O_DIRECTORY', 0))
        try: os.fsync(directory)
        finally: os.close(directory)
    finally:
        if os.path.exists(temporary): os.unlink(temporary)


def restore_before_start(data_dir, store, keys, vault_identity_file, *, stopped=services_stopped, now=None):
    """Return a start decision only after a complete restore; never start services."""
    data = Path(data_dir)
    def verify_stopped():
        try:
            return stopped() is True
        except Exception:
            return False
    if data.is_symlink() or not verify_stopped():
        raise RestoreFailure('services_not_stopped')
    runtime = data / '.backup-runtime'
    if runtime.is_symlink():
        raise RestoreFailure('unsafe_data_directory')
    data.mkdir(mode=0o700, parents=True, exist_ok=True)
    runtime.mkdir(mode=0o700, parents=True, exist_ok=True)
    flags = os.O_RDWR | os.O_CREAT | getattr(os, 'O_NOFOLLOW', 0) | getattr(os, 'O_CLOEXEC', 0)
    try:
        with os.fdopen(os.open(runtime / 'lock', flags, 0o600), 'a+b') as lock:
            try:
                fcntl.flock(lock, fcntl.LOCK_EX | fcntl.LOCK_NB)
            except BlockingIOError:
                raise RestoreFailure('backup_busy') from None
            if not verify_stopped():
                raise RestoreFailure('services_not_stopped')
            # A missing bucket and an empty bucket are not the same thing.
            store.bucket_retention()
            selected = backup.select_restore_candidate(store, CheckedKeys(keys))
            if selected is None:
                # Recheck both bucket access and slots so a disappearing bucket
                # or a concurrently arriving backup is never treated as empty.
                store.bucket_retention()
                if backup.select_restore_candidate(store, CheckedKeys(keys)) is not None:
                    raise RestoreFailure('backup_changed')
                return {'restored': False, 'restoredBackupId': None, 'restoredAt': None}
            slot, version = selected
            from_temporary = slot == backup.SLOT_COUNT
            if from_temporary:
                # Only after all regular slots failed verification, promote the
                # verified temporary copy. If interrupted, the next start can
                # retry promotion before choosing a restore source.
                slot = backup._finish_staged_backup(store, CheckedKeys(keys), backup.DATABASE_AND_VAULT)
                version = store.read_versioned(slot)
                if not version:
                    raise RestoreFailure('backup_changed')
                selected = slot, version
            if not version or not re.fullmatch(r'[0-9]{1,32}', version[0]):
                raise RestoreFailure('backup_unreadable')
            def verify_identity(identity):
                path = Path(vault_identity_file)
                if path.parent.is_symlink() or path.is_symlink() or not path.is_file():
                    return 'conflict'
                if len(identity) > 1024 or path.stat().st_size > 1024:
                    return 'conflict'
                return 'already_present' if hmac.compare_digest(path.read_bytes().strip(), identity.strip()) else 'conflict'
            result = backup.restore(data, store, CheckedKeys(keys), vault_identity_writer=verify_identity, archive=data / 'archive', slot=slot)
            if result.get('status') == 'no_backup':
                raise RestoreFailure('backup_changed')
            if result.get('status') == 'conflict' or result.get('conflicts'):
                raise RestoreFailure('restore_conflict')
            if result.get('status') != 'restored':
                raise RestoreFailure('backup_unreadable')
            after = store.read_versioned(slot)
            if backup.select_restore_candidate(store, CheckedKeys(keys)) != selected or not after or after[0] != version[0]:
                raise RestoreFailure('backup_changed')
            protect_first_backup(data, f'slot-{slot}@{version[0]}')
            stamp = now or datetime.datetime.now(datetime.timezone.utc)
            result = {'restored': True, 'restoredBackupId': f'slot-{slot}@{version[0]}', 'restoredAt': stamp.isoformat().replace('+00:00', 'Z')}
            if from_temporary:
                result['restoredFromTemporary'] = True
            return result
    except RestoreFailure:
        raise
    except Exception as error:
        # Never include cloud response bodies, paths, credentials or token data.
        name = type(error).__name__
        if str(error) == 'backup_changed':
            detail = 'backup_changed'
        elif 'vault_identity_conflict' in str(error) or 'vault_identity_unavailable' in str(error):
            detail = 'key_unavailable'
        elif name == 'InvalidTag' or 'integrity error' in str(error):
            detail = 'checksum_mismatch'
        else:
            detail = 'backup_unreadable'
        raise RestoreFailure(detail) from None


def write_report(path, value):
    path = Path(path)
    if path.is_symlink():
        raise RestoreFailure('unsafe_report')
    path.parent.mkdir(mode=0o700, parents=True, exist_ok=True)
    fd, name = tempfile.mkstemp(prefix='.momso-restore-result-', dir=path.parent)
    try:
        with os.fdopen(fd, 'w', encoding='utf-8') as output:
            json.dump(value, output, separators=(',', ':'))
            output.flush()
            os.fsync(output.fileno())
        os.replace(name, path)
    finally:
        if os.path.exists(name): os.unlink(name)


def main(argv=None):
    parser = argparse.ArgumentParser()
    for name in ('installation-id', 'data-dir', 'bucket', 'kms-key', 'vault-identity', 'report'):
        parser.add_argument('--' + name, required=True)
    args = parser.parse_args(argv)
    # Run as the server user, with the identity provided by systemd's
    # LoadCredential; restored files must remain readable by that user.
    if (args.data_dir != '/var/lib/momso'
            or args.vault_identity != '/run/credentials/momso-restore.service/momso-vault-identity'
            or args.report != '/var/lib/momso/.momso-prestart-restore.json'):
        print(json.dumps({'ok': False, 'code': 'restore_failed', 'detail': 'unsafe_configuration'}, separators=(',', ':')))
        return 1
    try:
        if os.geteuid() != pwd.getpwnam('momso').pw_uid:
            raise RestoreFailure('unsafe_user')
        client = GoogleClient()
        verify_cloud_ownership(client, args.bucket, args.kms_key, args.installation_id)
        value = restore_before_start(args.data_dir, CloudStorage(client, args.bucket), CloudKms(client, args.kms_key), args.vault_identity)
        write_report(args.report, {'ok': True, **value})
        print(json.dumps({'ok': True, **value}, separators=(',', ':')))
        return 0
    except Exception as error:
        detail = error.detail if isinstance(error, RestoreFailure) else 'backup_unreadable'
        value = {'ok': False, 'code': 'restore_failed', 'detail': detail}
        try: write_report(args.report, value)
        except Exception: pass
        print(json.dumps(value, separators=(',', ':')))
        return 1


if __name__ == '__main__': sys.exit(main())
