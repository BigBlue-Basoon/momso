#!/usr/bin/env python3
"""Separate desktop latest from anywhere releases; publishing is explicit."""
import argparse
import json
import re
import subprocess
import urllib.error
import urllib.request

API = "https://api.github.com/repos/BigBlue-Basoon/momso/releases/latest"


def verify_desktop(release, tag):
    if not re.fullmatch(r"v\d+\.\d+\.\d+", tag):
        raise ValueError("desktop tag must be v followed by a stable version")
    if release.get("draft") is not False or release.get("prerelease") is not False or release.get("tag_name") != tag:
        raise ValueError("latest is not the expected desktop release")
    assets = {asset.get("name") for asset in release.get("assets", [])
              if isinstance(asset, dict) and asset.get("state") == "uploaded"}
    installer = f"momso_{tag[1:]}_x64-setup.exe"
    if not {installer, "latest.json"}.issubset(assets):
        raise ValueError("desktop installer or latest.json is missing")
    return installer


def anywhere_command(tag, assets, prerelease=False):
    if (not re.fullmatch(r"anywhere-[A-Za-z0-9][A-Za-z0-9._-]*", tag)
            or not assets or any(not asset or asset.startswith("-") for asset in assets)):
        raise ValueError("an anywhere tag and release files are required")
    command = ["gh", "release", "create", tag, *assets, "--repo", "BigBlue-Basoon/momso",
               "--verify-tag", "--latest=false"]
    if prerelease:
        command.append("--prerelease")
    return command


def main():
    parser = argparse.ArgumentParser()
    sub = parser.add_subparsers(dest="action", required=True)
    check = sub.add_parser("verify-desktop")
    check.add_argument("--tag", required=True)
    create = sub.add_parser("publish-anywhere")
    create.add_argument("--tag", required=True)
    create.add_argument("--prerelease", action="store_true")
    create.add_argument("assets", nargs="+")
    args = parser.parse_args()
    if args.action == "publish-anywhere":
        subprocess.run(anywhere_command(args.tag, args.assets, args.prerelease), check=True)
        return
    request = urllib.request.Request(API, headers={"User-Agent": "momso-release-check", "Accept": "application/vnd.github+json"})
    with urllib.request.urlopen(request, timeout=30) as response:
        release = json.load(response)
    print("Verified desktop latest:", args.tag, verify_desktop(release, args.tag))


if __name__ == "__main__":
    try:
        main()
    except (ValueError, OSError, urllib.error.URLError, subprocess.CalledProcessError) as error:
        raise SystemExit(f"Release channel check failed: {error}")
