"""Create a reproducible public VM bundle from explicitly supplied build artifacts."""
import argparse
import gzip
import hashlib
import tarfile
from pathlib import Path


def digest(path):
    value = hashlib.sha256()
    with path.open("rb") as f:
        for block in iter(lambda: f.read(1024 * 1024), b""):
            value.update(block)
    return value.hexdigest()


def prepare(server, expected, web, output):
    server, web, output = server.resolve(), web.resolve(), output.resolve()
    if not server.is_file() or digest(server) != expected or not (web / "index.html").is_file():
        raise ValueError("server hash or web build is invalid")
    if output == server or output == web or web in output.parents:
        raise ValueError("output overlaps an input")
    entries = [(server, "momso-server")]
    scripts = Path(__file__).resolve().parent
    entries += [(scripts / name, name) for name in ("momso_mobile_proxy.py", "momso_anywhere_vm_proxy.py")]
    entries += [(scripts / name, name) for name in ("momso_anywhere_backup.py", "momso_anywhere_backup_agent.py", "momso_anywhere_backup_google.py", "momso_anywhere_vault_identity.py")]
    tauri = scripts.parent / "apps/alpha/desktop/src-tauri"
    entries += [(tauri / "momso-vault-provision/provision.py", "vault-provision.py"),
                (tauri / "momso-idle-stop/idle_stop.py", "idle-stop.py")]
    for path in sorted(web.rglob("*")):
        if path.is_symlink():
            raise ValueError("web build contains a symbolic link")
        if path.is_file():
            entries.append((path, "web/" + path.relative_to(web).as_posix()))
    with output.open("xb") as target:
        with gzip.GzipFile(fileobj=target, mode="wb", mtime=0, filename="") as compressed:
            with tarfile.open(fileobj=compressed, mode="w") as archive:
                for path, name in entries:
                    info = tarfile.TarInfo(name)
                    info.size = path.stat().st_size
                    info.mode = 0o755 if name == "momso-server" else 0o644
                    info.mtime = info.uid = info.gid = 0
                    with path.open("rb") as source:
                        archive.addfile(info, source)
    return digest(output)


def main():
    parser = argparse.ArgumentParser()
    parser.add_argument("--server", type=Path, required=True)
    parser.add_argument("--server-sha256", required=True)
    parser.add_argument("--web", type=Path, required=True)
    parser.add_argument("--output", type=Path, required=True)
    args = parser.parse_args()
    try:
        print("SHA-256: " + prepare(args.server, args.server_sha256.lower(), args.web, args.output))
    except (OSError, ValueError) as error:
        print("Failed: " + str(error))
        return 1
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
