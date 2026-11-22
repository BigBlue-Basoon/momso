"""Transactional installation of signed release helpers and the next updater."""
import hashlib
import json
import os
from pathlib import Path
import re
import shutil
import tempfile

import momso_managed_files as managed

HELPERS = ("idle-stop.py", "vault-provision.py", "momso_anywhere_backup.py",
           "momso_anywhere_backup_agent.py", "momso_anywhere_backup_google.py",
           "momso_anywhere_vault_identity.py")
WORKER = ("update.py", "release_manifest.py", "server_admission.py", "systemd_host.py",
          "data_snapshots.py", "cloud_run_gateway.py", "github_releases.py", "progress_status.py",
          "layout_migration.py", "install_worker.py", "worker.py", "ensure_gcloud.py",
          "managed_helpers.py", "momso_managed_files.py")
LAUNCHER = b'''"""Stable launcher for the currently selected updater version."""\nimport runpy\nimport sys\nfrom pathlib import Path\nroot = Path(__file__).resolve().parent\ncurrent = (root / "current").resolve(strict=True)\nversions = (root / "versions").resolve(strict=True)\nif current.parent != versions or not current.is_dir():\n    raise RuntimeError("invalid_updater_version")\nsys.path.insert(0, str(current))\nrunpy.run_path(str(current / "worker.py"), run_name="__main__")\n'''


def _sync_dir(path):
    fd = os.open(path, os.O_RDONLY | os.O_DIRECTORY)
    try: os.fsync(fd)
    finally: os.close(fd)


def _atomic(path, data):
    fd, name = tempfile.mkstemp(prefix=".s108-", dir=path.parent)
    try:
        with os.fdopen(fd, "wb") as stream:
            stream.write(data); stream.flush(); os.fsync(stream.fileno())
        os.replace(name, path)
        fd = os.open(path.parent, os.O_RDONLY | os.O_DIRECTORY)
        try: os.fsync(fd)
        finally: os.close(fd)
    finally:
        if os.path.exists(name): os.unlink(name)


class ManagedHelpers:
    def __init__(self, flat=Path('/opt/momso'), worker=Path('/opt/momso-updater'),
                 private=Path('/etc/momso-private'), units=Path('/etc/systemd/system')):
        self.flat, self.worker, self.private, self.units = map(Path, (flat, worker, private, units))
        self.pointer = self.worker / 'current'
        self.pointer_journal = self.private / 'managed-s108-pointer-journal.json'
        self.changed_worker = False
        self.changed_units = False
        self.changed_idle = False

    def _owner(self):
        owners = []
        for name in ('managed-files.json', 'managed-update-worker.json'):
            path = self.private / name
            if path.is_symlink() or not path.is_file(): raise ValueError('managed_manifest_missing')
            data = json.loads(path.read_text(encoding='utf8'))
            if data.get('version') != 1 or not re.fullmatch(r'[a-f0-9]{12}', data.get('owner', '')):
                raise ValueError('managed_manifest_invalid')
            owners.append(data['owner'])
        if owners[0] != owners[1]: raise ValueError('managed_owner_mismatch')
        return owners[0]

    def _groups(self):
        return [(self.flat, self.private / 'managed-files.json', self.private / 'managed-s108-files-journal.json'),
                (self.units, self.private / 'managed-units.json', self.private / 'managed-s108-units-journal.json'),
                (self.worker, self.private / 'managed-update-worker.json', self.private / 'managed-s108-worker-journal.json'),
                (self.units / 'momso-server.service.d', self.private / 'managed-overrides.json', self.private / 'managed-s108-overrides-journal.json')]

    def apply(self, release):
        release = Path(release)
        history_file = release / 'momso-managed-history.json'
        if not history_file.is_file(): return False  # Older signed releases.
        if history_file.is_symlink(): raise ValueError('invalid_release_helpers')
        if not (release / 'momso-updater/managed_helpers.py').exists(): return False
        history = json.loads(history_file.read_text(encoding='utf8'))
        if history.get('version') != 1: raise ValueError('invalid_release_helpers')
        owner = self._owner()
        if os.path.lexists(self.pointer_journal) or any(os.path.lexists(journal) for _, _, journal in self._groups()):
            raise ValueError('managed_recovery_required')
        def files(directory, names, *, required=False):
            result = {}
            for name in names:
                path = directory / name
                if not path.exists() and not required: continue
                if path.is_symlink() or not path.is_file() or path.stat().st_size > 8 * 1024 * 1024:
                    raise ValueError('invalid_release_helpers')
                result[name] = path.read_bytes()
            return result
        helper_files = files(release, HELPERS)
        worker_files = files(release / 'momso-updater', WORKER, required=True)
        units = files(release / 'momso-units', ('momso-idle-stop.service',))
        overrides = files(release / 'momso-overrides', ('vault.conf',))
        installed_unit = self.units / 'momso-idle-stop.service'
        if installed_unit.is_symlink(): raise ValueError('managed_unit_conflict')
        if not installed_unit.exists(): units = {}
        installed_override = self.units / 'momso-server.service.d' / 'vault.conf'
        if installed_override.is_symlink(): raise ValueError('managed_unit_conflict')
        if not installed_override.exists(): overrides = {}
        for group, values in (('files', helper_files), ('worker', worker_files)):
            legacy = history.get(group, {})
            for name, content in values.items():
                if hashlib.sha256(content).hexdigest() not in legacy.get(name, []):
                    raise ValueError('invalid_release_helpers')
        versions = self.worker / 'versions'
        versions.mkdir(mode=0o755, exist_ok=True)
        if versions.is_symlink(): raise ValueError('invalid_updater_version')
        digest = hashlib.sha256()
        for name, content in sorted(worker_files.items()):
            digest.update(name.encode()); digest.update(b'\0'); digest.update(hashlib.sha256(content).digest())
        version = versions / digest.hexdigest()
        if not version.exists():
            temp = Path(tempfile.mkdtemp(prefix='.s108-version-', dir=versions))
            try:
                os.chmod(temp, 0o755)
                for name, content in worker_files.items():
                    with (temp / name).open('wb') as stream:
                        stream.write(content); stream.flush(); os.fsync(stream.fileno())
                    os.chmod(temp / name, 0o644)
                fd = os.open(temp, os.O_RDONLY | os.O_DIRECTORY)
                try: os.fsync(fd)
                finally: os.close(fd)
                os.rename(temp, version)
                fd = os.open(versions, os.O_RDONLY | os.O_DIRECTORY)
                try: os.fsync(fd)
                finally: os.close(fd)
            finally:
                if temp.exists(): shutil.rmtree(temp)
        if version.is_symlink() or not version.is_dir(): raise ValueError('invalid_updater_version')
        for name, content in worker_files.items():
            path = version / name
            if path.is_symlink() or not path.is_file() or path.read_bytes() != content: raise ValueError('invalid_updater_version')
        old = os.readlink(self.pointer) if self.pointer.is_symlink() else None
        if self.pointer.exists() and old is None: raise ValueError('invalid_updater_version')
        new = 'versions/' + digest.hexdigest()
        if old is not None and not re.fullmatch(r'versions/[a-f0-9]{64}', old): raise ValueError('invalid_updater_version')
        if old is not None and (self.pointer.resolve(strict=True).parent != versions.resolve(strict=True) or not self.pointer.resolve(strict=True).is_dir()):
            raise ValueError('invalid_updater_version')
        self.changed_worker = old != new
        self.changed_idle = 'idle-stop.py' in helper_files and (not (self.flat / 'idle-stop.py').is_file() or (self.flat / 'idle-stop.py').read_bytes() != helper_files['idle-stop.py'])
        self.changed_units = (bool(units) and installed_unit.read_bytes() != units['momso-idle-stop.service']) or (bool(overrides) and installed_override.read_bytes() != overrides['vault.conf'])
        groups = self._groups()
        try:
            if helper_files: managed.install(*groups[0], owner, helper_files, history.get('files', {}), 0o755)
            if units: managed.install(*groups[1], owner, units, {}, 0o644)
            if overrides: managed.install(*groups[3], owner, overrides, {}, 0o644)
            _atomic(self.pointer_journal, json.dumps({'owner': owner, 'old': old, 'new': new}).encode())
            next_link = self.worker / '.s108-current-next'
            if os.path.lexists(next_link): raise ValueError('invalid_updater_version')
            next_link.symlink_to(new, target_is_directory=True)
            os.replace(next_link, self.pointer)
            _sync_dir(self.worker)
            managed.install(*groups[2], owner, {'worker.py': LAUNCHER}, history.get('worker', {}), 0o644)
        except Exception:
            self.rollback()
            raise
        return self.changed_worker

    def rollback(self):
        owner = self._owner()
        groups = self._groups()
        managed.rollback(*groups[2], owner)
        if os.path.lexists(self.pointer_journal):
            if self.pointer_journal.is_symlink(): raise ValueError('invalid_updater_version')
            record = json.loads(self.pointer_journal.read_text(encoding='utf8'))
            old, new = record.get('old'), record.get('new')
            if record.get('owner') != owner or not re.fullmatch(r'versions/[a-f0-9]{64}', new or '') or (old is not None and not re.fullmatch(r'versions/[a-f0-9]{64}', old)):
                raise ValueError('invalid_updater_version')
            current = os.readlink(self.pointer) if self.pointer.is_symlink() else None
            if current not in (old, new) or (self.pointer.exists() and current is None): raise ValueError('invalid_updater_version')
            next_link = self.worker / '.s108-current-next'
            if os.path.lexists(next_link):
                if not next_link.is_symlink() or os.readlink(next_link) not in (old, new): raise ValueError('invalid_updater_version')
                next_link.unlink()
            if old is None: self.pointer.unlink(missing_ok=True)
            else:
                next_link.symlink_to(old, target_is_directory=True)
                os.replace(next_link, self.pointer)
            _sync_dir(self.worker)
            self.pointer_journal.unlink()
            _sync_dir(self.private)
        managed.rollback(*groups[1], owner)
        managed.rollback(*groups[3], owner)
        managed.rollback(*groups[0], owner)

    def commit(self):
        owner = self._owner()
        for _, _, journal in self._groups():
            if journal.is_symlink(): raise ValueError('managed_journal_conflict')
            if journal.exists():
                record = json.loads(journal.read_text(encoding='utf8'))
                if record.get('version') != 1 or record.get('owner') != owner: raise ValueError('managed_journal_conflict')
        if self.pointer_journal.is_symlink(): raise ValueError('invalid_updater_version')
        if self.pointer_journal.exists():
            record = json.loads(self.pointer_journal.read_text(encoding='utf8'))
            if record.get('owner') != owner: raise ValueError('invalid_updater_version')
        for _, _, journal in self._groups():
            journal.unlink(missing_ok=True)
        self.pointer_journal.unlink(missing_ok=True)
        _sync_dir(self.private)
