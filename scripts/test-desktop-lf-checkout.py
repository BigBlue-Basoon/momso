#!/usr/bin/env python3
"""Compare tracked backend text against a fresh autocrlf-enabled checkout."""
import subprocess
import tempfile
from pathlib import Path

ROOT = Path(__file__).resolve().parent.parent
TREES = ("apps/alpha/desktop/src-tauri/", "apps/alpha/desktop/scripts/", "scripts/", ".github/")
EXTENSIONS = {".rs", ".sql", ".json", ".md", ".toml", ".py"}


def selected(path):
    return (path.startswith(TREES[1:]) or
            (path.startswith(TREES[0]) and Path(path).suffix in EXTENSIONS))


def main():
    unstaged = subprocess.run(["git", "diff", "--quiet"], cwd=ROOT).returncode
    staged = subprocess.run(["git", "diff", "--cached", "--quiet"], cwd=ROOT).returncode
    if unstaged or staged:
        raise RuntimeError("Commit tracked changes before comparing the checkout")
    names = subprocess.check_output(["git", "ls-files", "-z"], cwd=ROOT).decode("utf-8").split("\0")
    files = [name for name in names if name and selected(name)]
    with tempfile.TemporaryDirectory(prefix="momso-lf-checkout-") as temp:
        checkout = Path(temp) / "checkout"
        subprocess.run(["git", "-c", "core.autocrlf=true", "clone", "--shared", "--quiet", str(ROOT), str(checkout)], check=True)
        differences = [name for name in files if (ROOT / name).read_bytes() != (checkout / name).read_bytes()]
        for name in differences:
            print(f"Different: {name}")
        if differences:
            raise RuntimeError(f"{len(differences)} checkout files differ")
        print(f"Fresh autocrlf checkout matches: {len(files)} files")


if __name__ == "__main__":
    main()
