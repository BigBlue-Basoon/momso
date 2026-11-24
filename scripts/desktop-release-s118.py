#!/usr/bin/env python3
"""Prepare local desktop release files; never publish or print signing secrets."""
import argparse
import getpass
import hashlib
import json
import os
from pathlib import Path
import re
import shutil
import subprocess
import sys
import tempfile

ROOT = Path(__file__).resolve().parent.parent
CONFIG = ROOT / "apps/alpha/desktop/src-tauri/tauri.conf.json"
CLI = ROOT / "node_modules/@tauri-apps/cli/tauri.js"
REPOSITORY = "https://github.com/BigBlue-Basoon/momso"


def private_terms(terms_file):
    terms = []
    username = os.environ.get("USERNAME")
    if username:
        terms.append("C:\\Users\\" + username + "\\")
    if terms_file:
        terms += [line.strip() for line in terms_file.read_text(encoding="utf-8").splitlines() if line.strip()]
    return terms


def check_installer(path, version, terms_file=None):
    expected = f"momso_{version}_x64-setup.exe"
    if path.name != expected or path.is_symlink() or not path.is_file():
        raise ValueError("Expected the single version-matched NSIS installer")
    data = path.read_bytes()
    if len(data) < 1024 or data[:2] != b"MZ":
        raise ValueError("Installer is not a Windows executable")
    for term in private_terms(terms_file):
        for encoding in ("utf-8", "utf-16-le"):
            if term.casefold().encode(encoding) in data.lower():
                raise ValueError("Installer contains a private path or supplied private term")
    return hashlib.sha256(data).hexdigest()


def check_bundle(installer, terms_file=None):
    """Inspect extracted files too: compressed NSIS data is not visible in the EXE bytes."""
    tool = shutil.which("7z")
    if not tool:
        local = ROOT / "node_modules/.bin/7z.exe"
        tool = str(local) if local.is_file() else None
    if not tool:
        raise ValueError("7-Zip is required to inspect the installer contents")
    terms = private_terms(terms_file)
    with tempfile.TemporaryDirectory(prefix="momso-s118-inspect-") as temp:
        result = subprocess.run([tool, "x", "-y", "-o" + temp, str(installer)],
                                stdout=subprocess.DEVNULL, stderr=subprocess.DEVNULL)
        if result.returncode:
            raise ValueError("Cannot extract installer; full 7-Zip with NSIS support is required")
        root = Path(temp)
        for file in root.rglob("*"):
            if file.is_symlink():
                raise ValueError("Installer contains a link")
            relative = str(file.relative_to(root)).casefold()
            if any(term.casefold() in relative for term in terms):
                raise ValueError("Installer contains a private name")
            if file.is_file():
                data = file.read_bytes().lower()
                if any(term.casefold().encode(enc) in data for term in terms for enc in ("utf-8", "utf-16-le")):
                    raise ValueError("Installer contains a private path or supplied private term")


def main():
    parser = argparse.ArgumentParser()
    parser.add_argument("action", choices=("prepare", "sign"))
    parser.add_argument("--installer", required=True, type=Path)
    parser.add_argument("--key-path", type=Path, help="Only the developer supplies this for sign")
    parser.add_argument("--private-terms-file", type=Path, required=True)
    args = parser.parse_args()
    if not any(line.strip() for line in args.private_terms_file.read_text(encoding="utf-8").splitlines()):
        raise ValueError("Private terms file must contain at least one term")
    config = json.loads(CONFIG.read_text(encoding="utf-8"))
    version = config["version"]
    if not re.fullmatch(r"\d+\.\d+\.\d+", version):
        raise ValueError("A stable application version is required")
    endpoint = config["plugins"]["updater"]["endpoints"][0]
    if endpoint != REPOSITORY + "/releases/latest/download/latest.json":
        raise ValueError("Unexpected updater endpoint")
    if args.installer.is_symlink():
        raise ValueError("Installer cannot be a link")
    installer = args.installer.resolve(strict=True)
    sha = check_installer(installer, version, args.private_terms_file)
    check_bundle(installer, args.private_terms_file)
    receipt = installer.parent / "momso-release-preparation.json"
    terms_hash = hashlib.sha256(args.private_terms_file.read_bytes()).hexdigest() if args.private_terms_file else None
    expected = {"version": version, "installer": installer.name, "sha256": sha, "privateTermsSha256": terms_hash}
    if args.action == "prepare":
        receipt.write_text(json.dumps(expected, indent=2) + "\n", encoding="utf-8")
        print("Prepared:", installer.name, "SHA-256:", sha)
        return
    if not args.key_path or not args.key_path.is_file():
        raise ValueError("Developer must provide a private key path")
    if json.loads(receipt.read_text(encoding="utf-8")) != expected:
        raise ValueError("Prepared installer changed; prepare it again")
    signature_path = Path(str(installer) + ".sig")
    if signature_path.exists() or (installer.parent / "latest.json").exists():
        raise ValueError("Signature or update metadata already exists; review before replacing it")
    env = os.environ.copy()
    env.pop("TAURI_SIGNING_PRIVATE_KEY", None)
    env.pop("TAURI_SIGNING_PRIVATE_KEY_PATH", None)
    env["TAURI_SIGNING_PRIVATE_KEY_PASSWORD"] = getpass.getpass("Signing key password (Enter if none): ")
    try:
        subprocess.run(["node", str(CLI), "signer", "sign", "--private-key-path", str(args.key_path), str(installer)],
                       cwd=ROOT, env=env, check=True, stdout=subprocess.DEVNULL, stderr=subprocess.DEVNULL)
    finally:
        env.pop("TAURI_SIGNING_PRIVATE_KEY_PASSWORD", None)
    signature = signature_path.read_text(encoding="utf-8").strip()
    if not signature or check_installer(installer, version, args.private_terms_file) != sha:
        raise ValueError("Signing did not produce a valid matching artifact")
    manifest = {"version": version, "platforms": {"windows-x86_64": {
        "signature": signature, "url": f"{REPOSITORY}/releases/download/v{version}/{installer.name}"}}}
    (installer.parent / "latest.json").write_text(json.dumps(manifest, indent=2) + "\n", encoding="utf-8")
    print("Signed files prepared; verify signature against app public key before publishing.")


if __name__ == "__main__":
    try:
        main()
    except (OSError, ValueError, subprocess.CalledProcessError, json.JSONDecodeError) as error:
        print(f"Release preparation failed: {error}", file=sys.stderr)
        sys.exit(1)
