"""Stopped-service data snapshots for the standalone Linux updater."""
from pathlib import Path
import os
import shutil
import tempfile


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
                os.replace(self.data, previous)
            try:
                os.replace(replacement, self.data)
            except Exception:
                if previous.exists() and not self.data.exists():
                    os.replace(previous, self.data)
                raise
            # The old directory is deliberately retained for operator inspection.
        except Exception:
            # Keep any already-moved previous data on failure.
            if not (temporary / "previous").exists():
                shutil.rmtree(temporary, ignore_errors=True)
            raise
