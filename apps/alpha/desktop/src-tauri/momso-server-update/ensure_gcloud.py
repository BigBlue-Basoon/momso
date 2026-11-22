"""Install the official Debian Cloud CLI package only when missing."""
import os
from pathlib import Path
import shutil
import subprocess
import tempfile
import urllib.request


def ensure():
    if shutil.which("gcloud"):
        return
    os.environ["DEBIAN_FRONTEND"] = "noninteractive"
    def run(*args):
        subprocess.run(args, check=True, stdout=subprocess.DEVNULL, stderr=subprocess.DEVNULL)
    run("apt-get", "update", "-qq")
    run("apt-get", "install", "-y", "-qq", "ca-certificates", "gnupg")
    keyring = Path("/usr/share/keyrings/cloud.google.gpg")
    source = Path("/etc/apt/sources.list.d/google-cloud-sdk.list")
    entry = "deb [signed-by=/usr/share/keyrings/cloud.google.gpg] https://packages.cloud.google.com/apt cloud-sdk main\n"
    if source.exists():
        if source.read_text() != entry:
            raise RuntimeError("gcloud_source_conflict")
    else:
        with urllib.request.urlopen("https://packages.cloud.google.com/apt/doc/apt-key.gpg", timeout=30) as response:
            public = response.read(65537)
        if len(public) > 65536:
            raise RuntimeError("gcloud_key_invalid")
        result = subprocess.run(["gpg", "--dearmor"], input=public, check=True, stdout=subprocess.PIPE, stderr=subprocess.DEVNULL)
        for path, value in ((keyring, result.stdout), (source, entry.encode())):
            fd, name = tempfile.mkstemp(prefix=".momso-gcloud-", dir=path.parent)
            try:
                with os.fdopen(fd, "wb") as stream:
                    stream.write(value); stream.flush(); os.fsync(stream.fileno())
                os.chmod(name, 0o644); os.replace(name, path)
            finally:
                if os.path.exists(name): os.unlink(name)
    run("apt-get", "update", "-qq")
    run("apt-get", "install", "-y", "-qq", "google-cloud-cli")
    if not shutil.which("gcloud"):
        raise RuntimeError("gcloud_unavailable")
