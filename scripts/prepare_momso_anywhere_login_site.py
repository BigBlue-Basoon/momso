"""Build a login-site directory from explicitly supplied public assets."""
import argparse
import json
import re
import shutil
from html.parser import HTMLParser
from pathlib import Path

from momso_anywhere_public_assets import from_web_archive, from_web_directory, write_assets


class Slots(HTMLParser):
    def __init__(self):
        super().__init__()
        self.button = self.login_button = self.error = self.boot = False

    def handle_starttag(self, tag, attrs):
        values = dict(attrs)
        self.button |= tag == "div" and values.get("id") == "google-button"
        self.login_button |= tag == "button" and values.get("id") == "google-login" and values.get("type") == "button"
        self.error |= tag == "p" and values.get("id") == "error" and values.get("role") == "alert"
        self.boot |= tag == "script" and values.get("type") == "module" and values.get("src") == "./boot.mjs"


def prepare(output, client_id, login_page_dir=None, boot=None, web=None):
    if not re.fullmatch(r"[A-Za-z0-9._-]+\.apps\.googleusercontent\.com", client_id):
        raise ValueError("invalid client ID")
    output = Path(output).resolve()
    root = Path(__file__).resolve().parent
    source = Path(login_page_dir).resolve() if login_page_dir else root / "momso_anywhere_login_site"
    if not source.is_dir() or source == output or source in output.parents or output.exists():
        raise ValueError("invalid source or output")
    paths = sorted(source.rglob("*"))
    if any(path.is_symlink() for path in paths):
        raise ValueError("symbolic link in login page")
    if not (source / "index.html").is_file():
        raise ValueError("missing login page")
    assets = None
    if web is not None:
        web = Path(web)
        assets = from_web_directory(web) if web.is_dir() else from_web_archive(web)
        if any((source / name).exists() for name in assets):
            raise ValueError("login page overlaps public assets")
    boot = Path(boot) if boot is not None else root / "momso_anywhere_login_boot.mjs"
    if not boot.is_file() or boot.is_symlink():
        raise ValueError("login boot module is not ready")
    if login_page_dir:
        slots = Slots()
        slots.feed((source / "index.html").read_text(encoding="utf-8"))
        if not all((slots.button, slots.login_button, slots.error, slots.boot)) or not (source / "login.css").is_file():
            raise ValueError("login page contract is incomplete")
        if assets is None:
            raise ValueError("public web assets are required")
        if any((source / name).exists() for name in ("boot.mjs", "momso_anywhere_login.mjs", "config.json")):
            raise ValueError("login page overlaps generated files")
    output.mkdir(parents=True)
    for path in paths:
        destination = output / path.relative_to(source)
        if path.is_dir():
            destination.mkdir(exist_ok=True)
        elif path.is_file():
            destination.parent.mkdir(parents=True, exist_ok=True)
            shutil.copyfile(path, destination)
    shutil.copyfile(root / "momso_anywhere_login.mjs", output / "momso_anywhere_login.mjs")
    shutil.copyfile(boot, output / "boot.mjs")
    if assets is not None:
        write_assets(assets, output)
    config = {"clientId": client_id}
    (output / "config.json").write_text(json.dumps(config) + "\n", encoding="utf-8")


def main():
    parser = argparse.ArgumentParser()
    parser.add_argument("--output", type=Path, required=True)
    parser.add_argument("--client-id", required=True)
    parser.add_argument("--login-page-dir", type=Path)
    parser.add_argument("--boot-mjs", type=Path)
    parser.add_argument("--web", type=Path, help="verified web build directory or tar.gz")
    args = parser.parse_args()
    try:
        prepare(args.output, args.client_id, args.login_page_dir, args.boot_mjs, args.web)
    except (OSError, ValueError) as error:
        print("Failed: " + str(error))
        return 1
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
