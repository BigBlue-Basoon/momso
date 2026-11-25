"""Prepare a versioned baseline without changing the running flat installation."""
import json
import hashlib
import os
from pathlib import Path
import shutil
import tempfile

from update import UpdateError, _atomic_json, _download, _extract
from momso_anywhere_proxy_files import PROXY_FILES


def prepare_baseline(flat_root, update_root, installed_sequence):
    """Caller supplies the verified installed sequence and excludes concurrent writers."""
    if type(installed_sequence) is not int or installed_sequence < 1:
        raise ValueError("installed_sequence_required")
    flat, root = Path(flat_root), Path(update_root)
    if flat.is_symlink() or root.is_symlink():
        raise ValueError("invalid_layout")
    flat, root = flat.resolve(strict=True), root.resolve()
    if root == flat or root in flat.parents or root == flat / "web" or flat / "web" in root.parents:
        raise ValueError("overlapping_layout")
    root.mkdir(mode=0o755, parents=True, exist_ok=True)
    os.chmod(root, 0o755)
    current, sequence = root / "current", root / "installed.json"
    if sequence.exists():
        record = json.loads(sequence.read_text(encoding="utf-8"))
        if current.is_symlink():
            target = current.resolve(strict=True)
            if (root not in target.parents or type(record.get("sequence")) is not int
                    or record["sequence"] < installed_sequence or not target.is_dir()):
                raise ValueError("layout_already_initialized")
            return target
        baseline = record.get("baseline")
        if (record.get("sequence") != installed_sequence or not isinstance(baseline, str)
                or not baseline.startswith("baseline-") or Path(baseline).name != baseline):
            raise ValueError("layout_already_initialized")
        stage = root / baseline
        if stage.is_symlink() or not stage.is_dir():
            raise ValueError("baseline_unavailable")
        if current.exists():
            raise ValueError("layout_already_initialized")
        next_link = root / ".current-next"
        next_link.unlink(missing_ok=True)
        next_link.symlink_to(stage.name, target_is_directory=True)
        os.replace(next_link, current)
        return stage
    if current.exists() or current.is_symlink():
        raise ValueError("layout_already_initialized")
    files = ("momso-server", *PROXY_FILES)
    for name in files:
        source = flat / name
        if not source.is_file() or source.is_symlink():
            raise ValueError("invalid_flat_installation")
    web = flat / "web"
    if web.is_symlink() or not (web / ".momso-owned").is_file():
        raise ValueError("invalid_flat_installation")
    for directory, dirs, names in os.walk(web, followlinks=False):
        if any((Path(directory) / item).is_symlink() for item in dirs + names):
            raise ValueError("invalid_flat_installation")
    stage = Path(tempfile.mkdtemp(prefix="baseline-", dir=root))
    try:
        for name in files:
            shutil.copy2(flat / name, stage / name)
        shutil.copytree(web, stage / "web")
        os.chmod(stage, 0o755)
        # installed.json is written before the pointer: an interruption leaves
        # an explicitly incomplete layout, never a silently guessed sequence.
        _atomic_json(sequence, {"sequence": installed_sequence, "baseline": stage.name})
        next_link = root / ".current-next"
        next_link.symlink_to(stage.name, target_is_directory=True)
        os.replace(next_link, current)
        fd = os.open(root, os.O_RDONLY | os.O_DIRECTORY)
        try:
            os.fsync(fd)
        finally:
            os.close(fd)
        return stage
    except Exception:
        if not current.is_symlink():
            sequence.unlink(missing_ok=True)
            shutil.rmtree(stage, ignore_errors=True)
        raise


def prepare_verified_baseline(flat_root, updater, manifest_url, signature_url):
    """Identify the installed version from a signed, byte-for-byte matching bundle."""
    release = updater.check(manifest_url, signature_url)
    bundle = _download(release["bundleUrl"], updater.allow_http)
    if hashlib.sha256(bundle).hexdigest() != release["bundleSha256"]:
        raise UpdateError("digest_mismatch")
    flat = Path(flat_root)
    current = updater.root / "current"
    sequence = updater.root / "installed.json"
    existing = current.is_symlink() and sequence.is_file()
    if existing:
        target = current.resolve(strict=True)
        root = updater.root.resolve(strict=True)
        if root not in target.parents or json.loads(sequence.read_text(encoding="utf-8")).get("sequence") != release["sequence"]:
            raise UpdateError("installed_release_mismatch")
        flat = target
    with tempfile.TemporaryDirectory(prefix="momso-baseline-check-") as tmp:
        extracted = Path(tmp)
        _extract(bundle, extracted)
        # The installer may have independently refreshed its proxy scripts;
        # the server and web bytes identify the signed installed release.
        names = ("momso-server",)
        for name in names:
            source = flat / name
            if source.is_symlink() or not source.is_file() or source.read_bytes() != (extracted / name).read_bytes():
                raise UpdateError("installed_release_mismatch")
        installed_web, release_web = flat / "web", extracted / "web"
        def paths(root):
            return {p.relative_to(root) for p in root.rglob("*") if p.is_file() and p.name != ".momso-owned"}
        if installed_web.is_symlink() or not installed_web.is_dir() or paths(installed_web) != paths(release_web):
            raise UpdateError("installed_release_mismatch")
        for relative in paths(release_web):
            candidate = installed_web / relative
            if candidate.is_symlink() or candidate.read_bytes() != (release_web / relative).read_bytes():
                raise UpdateError("installed_release_mismatch")
        return flat if existing else prepare_baseline(flat, updater.root, release["sequence"])
