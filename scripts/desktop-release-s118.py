#!/usr/bin/env python3
"""Prepare local desktop release files; never publish or print signing secrets."""
import argparse
import getpass
import hashlib
import json
import os
from pathlib import Path
import re
import subprocess
import sys

ROOT = Path(__file__).resolve().parent.parent
CONFIG = ROOT / "apps/alpha/desktop/src-tauri/tauri.conf.json"
CLI = ROOT / "node_modules/@tauri-apps/cli/tauri.js"
REPOSITORY = "https://github.com/BigBlue-Basoon/momso"
PUBLIC_REPOSITORY = "https://github.com/BigBlue-Basoon/momso.git"


def verify_public_commit(repo, sha):
    if not re.fullmatch(r"[0-9a-f]{40}", sha or ""):
        raise ValueError("A full public source commit is required")
    def git(*args):
        return subprocess.run(["git", "-C", str(repo), *args], check=True,
                              capture_output=True, text=True).stdout.strip()
    if git("remote", "get-url", "origin").rstrip("/") != PUBLIC_REPOSITORY:
        raise ValueError("Unexpected public repository remote")
    git("fetch", "--no-tags", "origin")
    remote = git("ls-remote", "--exit-code", "origin", "refs/heads/main").split()[0]
    if git("rev-parse", "refs/remotes/origin/main") != remote:
        raise ValueError("Public main moved; fetch and retry")
    git("merge-base", "--is-ancestor", sha, remote)


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


def check_tree(root, terms_file):
    """Inspect installed and staged bytes without writing private terms to logs."""
    if root.is_symlink() or root.is_junction() or not root.is_dir():
        raise ValueError("Inspection directory is missing or is a link")
    terms = private_terms(terms_file)
    patterns = [term.casefold().encode(enc) for term in terms for enc in ("utf-8", "utf-16-le")]
    key_pattern = re.compile(rb"(?:sk-ant-|sk-proj-|ghp_|github_pat_)[a-z0-9_-]{16,}", re.I)
    key_pattern_wide = re.compile(rb"(?:s\x00k\x00-\x00a\x00n\x00t\x00-\x00|s\x00k\x00-\x00p\x00r\x00o\x00j\x00-\x00|g\x00h\x00p\x00_\x00|g\x00i\x00t\x00h\x00u\x00b\x00_\x00p\x00a\x00t\x00_\x00)(?:[a-z0-9_-]\x00){16,}", re.I)
    count = 0
    for file in root.rglob("*"):
        if file.is_symlink() or file.is_junction():
            raise ValueError("Inspection tree contains a link")
        relative = str(file.relative_to(root)).casefold()
        if any(term.casefold() in relative for term in terms):
            raise ValueError("Inspection tree contains a private name")
        if file.is_file():
            data = file.read_bytes()
            if any(pattern in data.lower() for pattern in patterns) or key_pattern.search(data) or key_pattern_wide.search(data):
                raise ValueError("Inspection tree contains a private path, term, or key pattern")
            count += 1
    if not count:
        raise ValueError("Inspection directory is empty")
    return count


def signer_tree_hash(root):
    if root.is_symlink() or root.is_junction() or not root.is_dir():
        raise ValueError('Prepared signing tool is missing')
    digest = hashlib.sha256()
    files = sorted(p for p in root.rglob('*') if p.is_file() and not p.is_symlink())
    if not files or any(p.is_symlink() or p.is_junction() for p in root.rglob('*')):
        raise ValueError('Prepared signing tool is incomplete or contains a link')
    for path in files:
        name = path.relative_to(root).as_posix().encode('utf8')
        digest.update(len(name).to_bytes(8, 'big')); digest.update(name)
        digest.update(hashlib.sha256(path.read_bytes()).digest())
    return digest.hexdigest()


def resolve_signer(installer, receipt):
    root = installer.parent / 'signer'
    if signer_tree_hash(root) != receipt.get('signerTreeSha256'):
        raise ValueError('Prepared signing tool changed')
    node = root / 'node.exe'
    cli = root / 'node_modules/@tauri-apps/cli/tauri.js'
    if not node.is_file() or not cli.is_file():
        raise ValueError('Prepared signing tool is incomplete')
    return node, cli


def check_bundle(installed_dir, staged_dirs, terms_file, built_exe):
    if not (installed_dir / "momso-desktop.exe").is_file() or not (installed_dir / "node/node.exe").is_file():
        raise ValueError("The installed product and bundled Node must both be present")
    original = built_exe.read_bytes()
    installed = (installed_dir / "momso-desktop.exe").read_bytes()
    before, after = b"__TAURI_BUNDLE_TYPE_VAR_UNK", b"__TAURI_BUNDLE_TYPE_VAR_NSS"
    if installed != original:
        if original.count(before) != 1 or installed != original.replace(before, after, 1):
            raise ValueError("Installed application does not match the built executable")
    node = (installed_dir / "node/node.exe").read_bytes()
    candidates = [path / "node.exe" for path in staged_dirs if (path / "node.exe").is_file()]
    if not candidates or not any(path.read_bytes() == node for path in candidates):
        raise ValueError("Installed Node does not match the staged runtime")
    return sum(check_tree(path, terms_file) for path in [installed_dir, *staged_dirs])


def main():
    parser = argparse.ArgumentParser()
    parser.add_argument("action", choices=("prepare", "record-startup", "sign"))
    parser.add_argument("--installer", required=True, type=Path)
    parser.add_argument("--key-path", type=Path, help="Only the developer supplies this for sign")
    parser.add_argument("--private-terms-file", type=Path, required=True)
    parser.add_argument("--installed-dir", type=Path)
    parser.add_argument("--staged-dir", type=Path, action="append", default=[])
    parser.add_argument("--built-exe", type=Path)
    parser.add_argument("--startup-evidence", type=Path)
    parser.add_argument("--public-repo", type=Path, default=Path.home() / "Documents/dev/momso-public")
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
    receipt = installer.parent / "momso-release-preparation.json"
    terms_hash = hashlib.sha256(args.private_terms_file.read_bytes()).hexdigest() if args.private_terms_file else None
    expected = {"version": version, "installer": installer.name, "sha256": sha, "privateTermsSha256": terms_hash}
    if args.action == "prepare":
        if not args.installed_dir or not args.staged_dir or not args.built_exe:
            raise ValueError("Installed product, built executable and staged source directories are required")
        expected["inspectedFiles"] = check_bundle(args.installed_dir, args.staged_dir, args.private_terms_file, args.built_exe)
        expected["startupVerified"] = False
        receipt.write_text(json.dumps(expected, indent=2) + "\n", encoding="utf-8")
        print("Prepared:", installer.name, "SHA-256:", sha)
        return
    saved = json.loads(receipt.read_text(encoding="utf-8"))
    if any(saved.get(key) != value for key, value in expected.items()) or not saved.get("inspectedFiles"):
        raise ValueError("Prepared installer changed; prepare it again")
    if args.action == "record-startup":
        if not args.startup_evidence:
            raise ValueError("Isolated product startup evidence is required")
        evidence = json.loads(args.startup_evidence.read_text(encoding="utf-8"))
        isolated = {"installerSha256": sha, "isolatedUser": True, "windowResponsive": True, "uninstalled": True}
        authorized = {"installerSha256": sha, "isolatedUser": False, "authorizedCurrentUser": True,
                      "windowResponsive": True, "firstScreenRendered": True, "uninstalled": True}
        if evidence not in (isolated, authorized):
            raise ValueError("Startup evidence does not match this installer")
        saved["startupVerified"] = True
        saved["startupMode"] = "isolated_user" if evidence == isolated else "authorized_current_user"
        receipt.write_text(json.dumps(saved, indent=2) + "\n", encoding="utf-8")
        print("Recorded product startup and uninstall evidence.")
        return
    if not saved.get("startupVerified"):
        raise ValueError("Isolated product startup and uninstall must be verified before signing")
    verify_public_commit(args.public_repo, saved.get("sourceCommitSha"))
    node, signer = resolve_signer(installer, saved)
    if not args.key_path or not args.key_path.is_file():
        raise ValueError("Developer must provide a private key path")
    signature_path = Path(str(installer) + ".sig")
    if signature_path.exists() or (installer.parent / "latest.json").exists():
        raise ValueError("Signature or update metadata already exists; review before replacing it")
    env = os.environ.copy()
    env.pop("TAURI_SIGNING_PRIVATE_KEY", None)
    env.pop("TAURI_SIGNING_PRIVATE_KEY_PATH", None)
    env["TAURI_SIGNING_PRIVATE_KEY_PASSWORD"] = getpass.getpass("Signing key password (Enter if none): ")
    try:
        subprocess.run([str(node), str(signer), "signer", "sign", "--private-key-path", str(args.key_path), str(installer)],
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
