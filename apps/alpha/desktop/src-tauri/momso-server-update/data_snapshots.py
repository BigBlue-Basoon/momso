"""Stopped-service data snapshots for the standalone Linux updater."""
from pathlib import Path
import os
import shutil
import tempfile
import fcntl
import stat
from contextlib import contextmanager

LOCK_NAMES = ('.momso-reservation-state.lock', '.momso-reservation-operation.lock')


def _open_lock(root, name):
    owner = root.stat().st_uid
    path = root / name
    try:
        fd = os.open(path, os.O_RDWR | os.O_NOFOLLOW | os.O_CLOEXEC)
    except FileNotFoundError:
        try:
            fd = os.open(path, os.O_RDWR | os.O_CREAT | os.O_EXCL | os.O_NOFOLLOW | os.O_CLOEXEC, 0o600)
            try:
                os.fchown(fd, owner, -1)
            except Exception:
                os.close(fd)
                raise
        except FileExistsError:
            fd = os.open(path, os.O_RDWR | os.O_NOFOLLOW | os.O_CLOEXEC)
    file = os.fdopen(fd, 'r+b', buffering=0)
    info = os.fstat(fd)
    if not stat.S_ISREG(info.st_mode) or info.st_nlink != 1 or info.st_uid != owner or info.st_mode & 0o077:
        file.close()
        raise ValueError('reservation_unavailable')
    return file


@contextmanager
def reserve_large_write(data_directory, requested_bytes):
    """Hold the server's large-write lock, including during snapshot copies."""
    root = Path(data_directory)
    handles = []
    try:
        for name in LOCK_NAMES:
            handles.append(_open_lock(root, name))
        state, operation = handles
        try:
            fcntl.flock(state, fcntl.LOCK_EX | fcntl.LOCK_NB)
            fcntl.flock(operation, fcntl.LOCK_EX | fcntl.LOCK_NB)
        except BlockingIOError:
            raise ValueError('reservation_busy') from None
        if shutil.disk_usage(root).free < 1536 * 1024 * 1024 + requested_bytes:
            raise ValueError('disk_low')
        operation.seek(0); operation.truncate(0)
        value = (str(requested_bytes) + '\n').encode('ascii')
        if operation.write(value) != len(value):
            raise ValueError('reservation_unavailable')
        os.fsync(operation.fileno())
        fcntl.flock(state, fcntl.LOCK_UN)
        yield
    finally:
        for file in reversed(handles): file.close()


def estimate_snapshot_bytes(data_directory):
    root = Path(data_directory)
    total = 0
    for directory, directories, files in os.walk(root, followlinks=False):
        if Path(directory) == root / 'codex':
            directories[:] = [name for name in directories if name != 'tmp']
            files[:] = [name for name in files if name != 'tmp']
        for name in files:
            path = Path(directory) / name
            if not path.is_symlink():
                total += path.stat().st_size
    return total


def _copy_tree(source, destination, *, exclude_codex_tmp=False):
    """Keep ownership when a root-run updater copies data for the service user."""
    source, destination = Path(source), Path(destination)
    def ignore(directory, names):
        return {"tmp"} if exclude_codex_tmp and Path(directory) == source / "codex" and "tmp" in names else set()
    shutil.copytree(source, destination, symlinks=True, copy_function=shutil.copy2, ignore=ignore)
    if os.name != "posix":
        return
    for current, directories, files in os.walk(source, followlinks=False):
        original = Path(current)
        if exclude_codex_tmp and original == source / "codex":
            directories[:] = [name for name in directories if name != "tmp"]
            files[:] = [name for name in files if name != "tmp"]
        for path in [original, *(original / name for name in directories + files)]:
            target = destination / path.relative_to(source)
            info = path.lstat()
            os.chown(target, info.st_uid, info.st_gid, follow_symlinks=False)


class DataSnapshots:
    def __init__(self, data_directory, snapshot_directory):
        self.data = Path(data_directory).resolve()
        self.snapshots = Path(snapshot_directory).resolve()
        if self.data == self.snapshots or self.data in self.snapshots.parents or self.snapshots in self.data.parents:
            raise ValueError("overlapping_data_and_snapshots")
        self.snapshots.mkdir(parents=True, exist_ok=True, mode=0o700)
        os.chmod(self.snapshots, 0o700)

    def snapshot(self):
        """Caller must stop every writer and keep maintenance admission closed."""
        if not self.data.is_dir():
            raise FileNotFoundError("data_directory_missing")
        # A link outside the data directory would otherwise be copied as a link,
        # leaving its target out of the rollback without warning.
        for current, directories, files in os.walk(self.data, followlinks=False):
            if Path(current) == self.data / "codex":
                directories[:] = [name for name in directories if name != "tmp"]
                files[:] = [name for name in files if name != "tmp"]
            for entry in directories + files:
                path = Path(current) / entry
                if path.is_symlink():
                    target = path.resolve()
                    if target != self.data and self.data not in target.parents:
                        raise ValueError("external_data_symlink")
        name = Path(tempfile.mkdtemp(prefix="data-", dir=self.snapshots))
        try:
            _copy_tree(self.data, name / "data", exclude_codex_tmp=True)
            return str(name)
        except Exception:
            shutil.rmtree(name)
            raise

    def restore(self, snapshot):
        """Copy before replacement; snapshot is retained for repeated crash recovery."""
        location = Path(snapshot).resolve()
        if location.parent != self.snapshots or not location.name.startswith("data-"):
            raise ValueError("invalid_snapshot")
        source = location / "data"
        if not source.is_dir() or source.is_symlink():
            raise ValueError("invalid_snapshot")
        temporary = Path(tempfile.mkdtemp(prefix=".momso-data-restore-", dir=self.data.parent))
        replacement = temporary / "data"
        try:
            _copy_tree(source, replacement)
            previous = temporary / "previous"
            if self.data.exists():
                with _open_lock(self.data, LOCK_NAMES[0]) as state:
                    with _open_lock(self.data, LOCK_NAMES[1]):
                        fcntl.flock(state, fcntl.LOCK_EX)
                        os.replace(self.data, previous)
                        moved = []
                        try:
                            for name in LOCK_NAMES:
                                os.replace(previous / name, replacement / name)
                                moved.append(name)
                            os.replace(replacement, self.data)
                        except Exception:
                            for name in reversed(moved):
                                os.replace(replacement / name, previous / name)
                            if not self.data.exists():
                                os.replace(previous, self.data)
                            raise
            else:
                os.replace(replacement, self.data)
            # The old directory is deliberately retained for operator inspection.
        except Exception:
            # Keep any already-moved previous data on failure.
            if not (temporary / "previous").exists():
                shutil.rmtree(temporary, ignore_errors=True)
            raise
