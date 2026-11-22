"""Package a separately built start screen and the public setup modules."""
import argparse
import base64
import json
import re
import shutil
from html.parser import HTMLParser
from pathlib import Path
from urllib.parse import urlsplit
import momso_release_keys


MODULES = ("momso_anywhere_start.mjs", "momso_anywhere_google_browser.mjs",
           "momso_anywhere_setup.mjs", "momso_anywhere_vm_startup.mjs", "momso_anywhere_cloud_run_revision.mjs",
           "momso_release_manifest.mjs",
           "momso_anywhere_oauth_capture.mjs", "momso_anywhere_oauth_redirect.mjs",
           "momso_anywhere_backup_setup.mjs")


class Assets(HTMLParser):
    def __init__(self):
        super().__init__()
        self.paths = []
        self.unsafe_script = False

    def handle_starttag(self, tag, attrs):
        values = dict(attrs)
        if tag == "script" and (values.get("type") != "module" or not values.get("src", "").startswith("/") or "async" in values):
            self.unsafe_script = True
        if tag in ("script", "link"):
            path = values.get("src") if tag == "script" else values.get("href")
            if path:
                self.paths.append(path)


def https_url(value, origin=False):
    parsed = urlsplit(value)
    if parsed.scheme != "https" or not parsed.hostname or parsed.username or parsed.password or parsed.query or parsed.fragment:
        return False
    if origin:
        return not parsed.path and parsed.netloc == parsed.hostname
    return parsed.netloc in (parsed.hostname, parsed.hostname + ":443") and parsed.path.startswith("/")


def prepare(dist, output, client_id, manifest_url, public_key, login_origin, trusted=(), channel='production'):
    dist, output = Path(dist).resolve(), Path(output).resolve()
    if not re.fullmatch(r"[A-Za-z0-9._-]+\.apps\.googleusercontent\.com", client_id):
        raise ValueError("invalid_client")
    image_pattern = r"[a-z0-9.-]+/[a-z0-9._/-]+@sha256:[a-f0-9]{64}"
    try:
        spki = momso_release_keys.validate(channel, public_key)
    except ValueError as error:
        raise ValueError('invalid_release') from error
    if not https_url(manifest_url) or not https_url(login_origin, origin=True) or any(not re.fullmatch(image_pattern, image) for image in trusted):
        raise ValueError("invalid_release")
    if not dist.is_dir() or not (dist / "index.html").is_file() or output.exists() or output == dist or dist in output.parents or output in dist.parents:
        raise ValueError("invalid_paths")
    if any(path.is_symlink() or (hasattr(path, "is_junction") and path.is_junction()) for path in dist.rglob("*")):
        raise ValueError("unsafe_build")
    if channel == 'production' and momso_release_keys.DEVELOPMENT_KEY.is_file():
        try:
            dev_key = momso_release_keys.public_der(momso_release_keys.DEVELOPMENT_KEY)
            encoded = base64.b64encode(dev_key)
            for path in dist.rglob('*'):
                if path.is_file():
                    content = path.read_bytes()
                    compact = b''.join(content.split())
                    if dev_key in content or encoded in compact:
                        raise ValueError('development_key_in_build')
        except OSError as error:
            raise ValueError('unsafe_build') from error
    if any((dist / name).exists() for name in (*MODULES, "config.json")):
        raise ValueError("build_overlap")
    parser = Assets()
    parser.feed((dist / "index.html").read_text(encoding="utf-8"))
    if parser.unsafe_script:
        raise ValueError("unsafe_build_script")
    if not any(path.startswith("/start/assets/") and path.endswith(".js") for path in parser.paths):
        raise ValueError("build_base_must_be_start")
    for path in parser.paths:
        if path.startswith("/"):
            relative = path.removeprefix("/start/")
            if not path.startswith("/start/") or "%" in relative or ".." in Path(relative).parts or not (dist / relative).is_file():
                raise ValueError("missing_build_asset")
        elif not path.startswith("https://"):
            raise ValueError("unexpected_build_asset")
    scripts = list((dist / "assets").glob("*.js"))
    if not scripts or not any("resumeAfterRedirect" in script.read_text(encoding="utf-8") for script in scripts):
        raise ValueError("screen_resume_missing")
    output.mkdir(parents=True)
    shutil.copytree(dist, output, dirs_exist_ok=True)
    html = (output / "index.html").read_text(encoding="utf-8")
    tag = '<script type="module" src="/start/momso_anywhere_oauth_capture.mjs"></script>\n'
    match = re.search(r'<script\b[^>]*\btype\s*=\s*["\']module["\']', html, flags=re.IGNORECASE)
    if not match:
        raise ValueError("missing_module_entry")
    (output / "index.html").write_text(html[:match.start()] + tag + html[match.start():], encoding="utf-8")
    scripts = Path(__file__).resolve().parent
    for name in MODULES:
        shutil.copyfile(scripts / name, output / name)
    release = {"manifestUrl": manifest_url, "publicKeySpki": base64.b64encode(spki).decode('ascii'), "loginOrigin": login_origin, "channel": channel}
    if trusted:
        release["trustedGatewayImages"] = list(trusted)
    (output / "config.json").write_text(json.dumps({"clientId": client_id, "release": release}, ensure_ascii=False) + "\n", encoding="utf-8")


def main():
    parser = argparse.ArgumentParser()
    parser.add_argument("--dist", type=Path, required=True)
    parser.add_argument("--output", type=Path, required=True)
    parser.add_argument("--client-id", required=True)
    parser.add_argument("--release-manifest-url", required=True)
    parser.add_argument("--release-public-key", type=Path, required=True)
    parser.add_argument("--release-channel", choices=['production', 'development'], default='production')
    parser.add_argument("--login-origin", required=True)
    parser.add_argument("--trusted-gateway-image", action="append", default=[])
    args = parser.parse_args()
    try:
        prepare(args.dist, args.output, args.client_id, args.release_manifest_url, args.release_public_key,
                args.login_origin, args.trusted_gateway_image, args.release_channel)
    except (OSError, ValueError) as error:
        reason = str(error)
        if reason not in {"invalid_client", "invalid_release", "invalid_paths", "unsafe_build", "development_key_in_build", "build_overlap", "unsafe_build_script", "build_base_must_be_start", "missing_build_asset", "unexpected_build_asset", "screen_resume_missing", "missing_module_entry"}:
            reason = "invalid_start_bundle"
        print("Failed: " + reason)
        return 1
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
