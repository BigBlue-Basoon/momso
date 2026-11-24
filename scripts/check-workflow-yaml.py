#!/usr/bin/env python3
"""Check every enabled GitHub workflow with the safe YAML parser."""
import sys
from pathlib import Path

import yaml


def check(directory):
    paths = sorted([*directory.glob("*.yml"), *directory.glob("*.yaml")])
    if not paths:
        raise ValueError("no workflow files")
    for path in paths:
        try:
            text = path.read_text(encoding="utf-8")
            document = yaml.safe_load(text)
            root = yaml.compose(text, Loader=yaml.SafeLoader)
        except (OSError, UnicodeError, yaml.YAMLError) as error:
            raise ValueError(f"{path}: {error}") from error
        # PyYAML follows YAML 1.1 and treats the unquoted key 'on' as True.
        if not isinstance(document, dict) or not isinstance(document.get("jobs"), dict) or not document["jobs"]:
            raise ValueError(f"{path}: missing jobs mapping")
        if not isinstance(root, yaml.MappingNode) or not any(key.value == "on" for key, _ in root.value):
            raise ValueError(f"{path}: missing on events")
        events = document.get("on", document.get(True))
        if not isinstance(events, (dict, list, str)) or not events:
            raise ValueError(f"{path}: missing on events")
    return len(paths)


def main():
    directory = Path(sys.argv[1]) if len(sys.argv) > 1 else Path(__file__).resolve().parent.parent / ".github/workflows"
    try:
        count = check(directory)
    except ValueError as error:
        print(error, file=sys.stderr)
        return 1
    print(f"workflow YAML: {count} passed")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
