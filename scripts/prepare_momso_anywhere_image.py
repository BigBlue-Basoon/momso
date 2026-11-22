"""Prepare a minimal image build context with public PWA files from a web build."""
import argparse
import shutil
from pathlib import Path

from momso_anywhere_public_assets import from_web_directory, write_assets


def prepare(web: Path, output: Path):
    assets = from_web_directory(web)
    output = output.resolve()
    if output == web.resolve() or web.resolve() in output.parents:
        raise ValueError("output overlaps web build")
    output.mkdir(parents=True, exist_ok=False)
    scripts = Path(__file__).resolve().parent
    for name in ("momso_anywhere_gateway.py", "momso_anywhere_auth.py", "momso_anywhere_login.mjs", "momso_anywhere_public_assets.py", "momso_wake/main.py", "momso_anywhere_image/Dockerfile"):
        target = output / "scripts" / name
        target.parent.mkdir(parents=True, exist_ok=True)
        shutil.copyfile(scripts / name, target)
    write_assets(assets, output / "public")


def main():
    parser = argparse.ArgumentParser()
    parser.add_argument("--web", type=Path, required=True)
    parser.add_argument("--output", type=Path, required=True)
    args = parser.parse_args()
    try:
        prepare(args.web, args.output)
    except (OSError, ValueError) as error:
        print("Failed: " + str(error))
        return 1
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
