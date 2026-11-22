"""Only product-supplied PWA metadata and icons may be public before login."""
import json
import tarfile
from pathlib import Path

ASSETS = (
    "manifest.webmanifest",
    "app-icons/apple-touch-icon.png",
    "app-icons/favicon-32.png",
    "app-icons/icon-192.png",
    "app-icons/icon-512.png",
    "app-icons/icon-512-maskable.png",
)


def validate(files):
    if set(files) != set(ASSETS):
        raise ValueError("PWA assets are missing")
    manifest = json.loads(files["manifest.webmanifest"])
    allowed = {"id", "name", "short_name", "lang", "start_url", "scope", "display", "background_color", "theme_color", "icons", "description"}
    if not isinstance(manifest, dict) or set(manifest) - allowed or manifest.get("start_url") != "/" or manifest.get("scope") != "/" or not isinstance(manifest.get("icons"), list):
        raise ValueError("PWA manifest is invalid")
    icons = {"/" + name for name in ASSETS if name.endswith(".png")}
    if not manifest["icons"] or any(not isinstance(icon, dict) or icon.get("src") not in icons for icon in manifest["icons"]):
        raise ValueError("PWA icon path is invalid")
    if any(not files[name].startswith(b"\x89PNG\r\n\x1a\n") for name in ASSETS if name.endswith(".png")):
        raise ValueError("PWA icon is invalid")


def from_web_directory(web: Path):
    root = web.resolve()
    files = {}
    for name in ASSETS:
        path = root / name
        if path.is_symlink() or not path.resolve().is_relative_to(root) or not path.is_file():
            raise ValueError("PWA asset is missing or unsafe")
        files[name] = path.read_bytes()
    validate(files)
    return files


def from_web_archive(archive: Path):
    files = {}
    with tarfile.open(archive, "r:gz") as tar:
        for member in tar:
            name = member.name.removeprefix("./")
            if name in ASSETS:
                if not member.isfile() or name in files:
                    raise ValueError("PWA asset is duplicated or unsafe")
                with tar.extractfile(member) as source:
                    files[name] = source.read()
    validate(files)
    return files


def write_assets(files, destination: Path):
    validate(files)
    for name, data in files.items():
        path = destination / name
        path.parent.mkdir(parents=True, exist_ok=True)
        path.write_bytes(data)
