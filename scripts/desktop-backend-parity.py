#!/usr/bin/env python3
"""Check or synchronize backend-owned files without touching screen or build files."""
import argparse
import json
import os
from pathlib import Path
import shutil
import sys

ROOT = Path(__file__).resolve().parent.parent
TREES = ("apps/alpha/desktop/src-tauri", ".github", "scripts", "apps/alpha/desktop/scripts")
INTEGRATION_ONLY_FILES = {
    ".github/pull_request_template.md",
    # The desktop snapshot does not contain the Linux server workspace.
    ".github/workflows/momso-server-linux.yml",
}
INTEGRATION_ONLY_TREES = {".github/ISSUE_TEMPLATE"}
SKIP_DIRS = {"target", "gen", "generated", "node_modules", ".git", "__pycache__"}
TRACKED_GENERATED_PLACEHOLDERS = ("apps/alpha/desktop/src-tauri/generated/node/.gitkeep",)
EXCEPTIONS = {
    ".github/workflows/desktop-alpha.yml": "The integration checkout also builds its separate website.",
    ".gitattributes": "The integration checkout additionally tracks a website LFS file; desktop LF rules must match.",
    ".gitignore": "The integration checkout has additional workspaces; Node placeholder rules must match.",
    "package.json": "The integration checkout contains additional workspaces.",
    "package-lock.json": "The integration checkout contains additional workspace dependencies.",
    "apps/alpha/desktop/package.json": "Frontend dependencies and tests belong to the screen checkout.",
}
OBSOLETE_SCREEN_FILES = {
    "apps/alpha/desktop/src-tauri/migrations/0008_archive_index.sql",
    "apps/alpha/desktop/src-tauri/migrations/0009_chats.sql",
    "apps/alpha/desktop/src-tauri/migrations/0010_chat_codex.sql",
    "apps/alpha/desktop/src-tauri/migrations/0011_chat_effort.sql",
    "apps/alpha/desktop/src-tauri/migrations/0012_chat_groups.sql",
    "apps/alpha/desktop/src-tauri/migrations/0013_chat_context.sql",
    "apps/alpha/desktop/scripts/prepare-mingit.mjs",
    "apps/alpha/desktop/scripts/prepare-mingit.test.mjs",
}


def inventory(root):
    files = {}
    for tree in TREES:
        directory = root / tree
        if not directory.exists():
            continue
        if directory.is_symlink():
            raise RuntimeError(f"Refusing symlinked directory: {tree}")
        for parent, dirs, names in os.walk(directory, followlinks=False):
            dirs[:] = [d for d in dirs if d not in SKIP_DIRS and
                       (Path(parent) / d).relative_to(root).as_posix() not in INTEGRATION_ONLY_TREES]
            for name in dirs:
                if (Path(parent) / name).is_symlink():
                    raise RuntimeError(f"Refusing symlinked directory: {Path(parent) / name}")
            for name in names:
                path = Path(parent) / name
                relative = path.relative_to(root).as_posix()
                if relative in EXCEPTIONS or relative in INTEGRATION_ONLY_FILES:
                    continue
                if path.is_symlink():
                    raise RuntimeError(f"Refusing symlink: {relative}")
                files[relative] = path
    for relative in TRACKED_GENERATED_PLACEHOLDERS:
        path = root / relative
        if path.is_symlink():
            raise RuntimeError(f"Refusing symlink: {relative}")
        if path.is_file():
            files[relative] = path
    return files


def lf_rules(root):
    path = root / ".gitattributes"
    if not path.is_file():
        return set()
    prefixes = ("apps/alpha/desktop/src-tauri/", "apps/alpha/desktop/scripts/", "scripts/", ".github/")
    return {line.strip() for line in path.read_text(encoding="utf-8").splitlines()
            if line.strip().startswith(prefixes)}


def check_ignore_rules(screen):
    source, target = ROOT / ".gitignore", screen / ".gitignore"
    if source.exists() != target.exists():
        return False
    if not source.exists():
        return True
    required = (
        "!apps/alpha/desktop/src-tauri/generated/node/",
        "apps/alpha/desktop/src-tauri/generated/node/*",
        "!apps/alpha/desktop/src-tauri/generated/node/.gitkeep",
    )
    for path in (source, target):
        lines = path.read_text(encoding="utf-8").splitlines()
        try:
            indexes = [lines.index(rule) for rule in required]
        except ValueError:
            return False
        if indexes != sorted(indexes):
            return False
    return True


def check_packages(screen):
    relative = "apps/alpha/desktop/package.json"
    source = json.loads((ROOT / relative).read_text(encoding="utf-8"))
    target = json.loads((screen / relative).read_text(encoding="utf-8"))
    required = "node scripts/prepare-node.mjs && npm run build"
    if any(source.get(key) != target.get(key) for key in ("version", "license")):
        return False
    if any(item.get("scripts", {}).get("build:bundle") != required for item in (source, target)):
        return False
    source_root, target_root = ROOT / "package.json", screen / "package.json"
    if source_root.exists() != target_root.exists():
        return False
    if source_root.exists():
        def desktop_scripts(path):
            values = json.loads(path.read_text(encoding="utf-8")).get("scripts", {})
            return {key: value for key, value in values.items() if key.startswith("desktop:")}
        return desktop_scripts(source_root) == desktop_scripts(target_root)
    return True


def check_workflow(screen):
    relative = ".github/workflows/desktop-alpha.yml"
    source, target = ROOT / relative, screen / relative
    if source.exists() != target.exists():
        return False
    if not source.exists():
        return True
    expected = source.read_text(encoding="utf-8").replace("'apps/alpha/**'", "'apps/alpha/desktop/**'")
    expected = expected.replace("      - name: Build explanation website\n        run: npm run alpha:build\n\n", "")
    return expected == target.read_text(encoding="utf-8")


def main():
    parser = argparse.ArgumentParser()
    parser.add_argument("screen", type=Path)
    parser.add_argument("--sync", action="store_true")
    args = parser.parse_args()
    screen = args.screen.resolve(strict=True)
    if screen == ROOT or ROOT in screen.parents or screen in ROOT.parents:
        raise RuntimeError("The screen checkout must be a separate directory.")
    source, target = inventory(ROOT), inventory(screen)
    extras = sorted(set(target) - set(source))
    unknown = [name for name in extras if name not in OBSOLETE_SCREEN_FILES]
    if unknown:
        for name in unknown:
            print(f"Unknown screen file (preserved): {name}")
        return 1
    if args.sync:
        for name in extras:
            (screen / name).unlink()
        for name, origin in source.items():
            destination = screen / name
            destination.parent.mkdir(parents=True, exist_ok=True)
            if destination.exists() and destination.is_symlink():
                raise RuntimeError(f"Refusing symlink: {name}")
            if not destination.exists() or origin.read_bytes() != destination.read_bytes():
                shutil.copyfile(origin, destination)
        target = inventory(screen)
    differences = []
    for name in sorted(INTEGRATION_ONLY_FILES | INTEGRATION_ONLY_TREES):
        if (screen / name).exists():
            differences.append(f"{name} (integration only; preserved)")
    for name in sorted(set(source) | set(target)):
        if name not in source or name not in target or source[name].read_bytes() != target[name].read_bytes():
            differences.append(name)
    if lf_rules(ROOT) != lf_rules(screen):
        differences.append(".gitattributes (desktop LF rules)")
    if not check_ignore_rules(screen):
        differences.append(".gitignore (Node placeholder rules)")
    if not check_packages(screen):
        differences.append("package.json (desktop version, license, or build contract)")
    if not check_workflow(screen):
        differences.append(".github/workflows/desktop-alpha.yml (other than website differences)")
    for name in differences:
        print(f"Different: {name}")
    for name, reason in EXCEPTIONS.items():
        print(f"Exception: {name}: {reason}")
    print(f"Backend files: {len(source)}; differences: {len(differences)}")
    return 1 if differences else 0


if __name__ == "__main__":
    try:
        sys.exit(main())
    except (OSError, RuntimeError, ValueError) as error:
        print(str(error), file=sys.stderr)
        sys.exit(1)
