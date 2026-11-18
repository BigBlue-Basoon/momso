#!/usr/bin/env python3
"""Check public source files without printing matched private values.

Optionally set MOMSO_PUBLIC_PRIVATE_TERMS_FILE to a UTF-8 file outside the
public tree containing one additional literal to reject per line.
"""
import os
import re
import sys
from pathlib import Path

ROOT = Path(__file__).resolve().parent.parent
SKIP = {".git", "node_modules", "target", "generated", "dist", "build", ".next", "coverage", "__pycache__"}
WINDOWS_USER = re.compile(r"(?i)\b[a-z]:[\\/]{1,2}users[\\/]{1,2}[^\\/\s\"'<>]+")
EMAIL = re.compile(r"(?i)\b[a-z0-9._%+\-]+@[a-z0-9.\-]+\.[a-z]{2,}\b")
KEY = re.compile(r"\b(?:sk-ant-|sk-proj-|ghp_|github_pat_)[a-zA-Z0-9_\-]{16,}\b")
FAKE_DOMAINS = {"example.com", "example.invalid", "example.test", "momso.local", "2x.png", "evil.test", "attacker.test"}
# Public copyright notices and one literal Windows Credential Manager target.
# Match both the exact file and exact value; all other checks still apply.
PUBLIC_ADDRESSES = {
    "apps/alpha/desktop/third-party/ego-tree-0.10.0-LICENSE": {"june" + "@" + "causal.agency"},
    "apps/alpha/desktop/third-party/scraper-0.24.0-LICENSE": {
        "june" + "@" + "causal.agency", "yoursvivek" + "@" + "gmail.com"
    },
    "scripts/momso-provider-smoke.py": {"api-key" + "@" + "app.momso.desktop"},
}


def files(root):
    for path in sorted(root.iterdir()):
        if path.name != ".git" and path.is_file() and not path.is_symlink():
            yield path
    for folder in (".github", "apps/alpha/desktop", "scripts"):
        top = root / folder
        if not top.is_dir():
            continue
        for directory, dirs, names in os.walk(top, followlinks=False):
            dirs[:] = [name for name in dirs if name not in SKIP and not (Path(directory) / name).is_symlink()]
            for name in names:
                path = Path(directory) / name
                if not path.is_symlink() and path.is_file():
                    yield path


def findings(root, terms):
    results = []
    for path in files(root):
        try:
            data = path.read_bytes()
            if b"\x00" in data:
                continue
            text = data.decode("utf-8")
        except (OSError, UnicodeDecodeError):
            continue
        relative = path.relative_to(root).as_posix()
        for number, line in enumerate(text.splitlines(), 1):
            kinds = []
            if WINDOWS_USER.search(line):
                kinds.append("user_path")
            if any(m.group().rsplit("@", 1)[1].lower() not in FAKE_DOMAINS
                   and m.group().lower() not in PUBLIC_ADDRESSES.get(relative, set())
                   for m in EMAIL.finditer(line)):
                kinds.append("email")
            if KEY.search(line):
                kinds.append("key")
            if any(term.casefold() in line.casefold() for term in terms):
                kinds.append("private_term")
            for kind in kinds:
                results.append((relative, number, kind))
    return results


def main():
    root = Path(sys.argv[1]).resolve() if len(sys.argv) > 1 else ROOT
    source = os.environ.get("MOMSO_PUBLIC_PRIVATE_TERMS_FILE")
    terms = []
    if source:
        location = Path(source).resolve(strict=True)
        if location.is_relative_to(root):
            raise SystemExit("private terms file must be outside the public tree")
        terms = [line.strip() for line in location.read_text(encoding="utf-8").splitlines() if line.strip()]
    matches = findings(root, terms)
    for path, number, kind in matches:
        print(f"{path}:{number}: {kind}")
    print(f"findings: {len(matches)}")
    return 1 if matches else 0


if __name__ == "__main__":
    raise SystemExit(main())
