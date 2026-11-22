"""Prepare an age vault identity for a replacement VM without logging it.

Not yet connected to the server startup or the backup archive.
"""
import os
import hmac
import subprocess
import tempfile
from pathlib import Path


def rewrap_for_host(identity, destination, *, run=subprocess.run):
    """Encrypt the identity to this host and create, but never replace, a credential."""
    destination = Path(destination)
    if not identity or destination.is_symlink():
        raise ValueError("invalid vault credential")
    if destination.exists():
        result = run(
            ["systemd-creds", "decrypt", "--no-ask-password", "--name=momso-vault-identity", str(destination), "-"],
            stdout=subprocess.PIPE, stderr=subprocess.DEVNULL, check=False,
        )
        return "already_present" if result.returncode == 0 and hmac.compare_digest(result.stdout, identity) else "conflict"
    result = run(
        ["systemd-creds", "encrypt", "--no-ask-password", "--name=momso-vault-identity", "-", "-"],
        input=identity, stdout=subprocess.PIPE, stderr=subprocess.DEVNULL, check=False,
    )
    if result.returncode != 0 or not result.stdout:
        raise RuntimeError("vault_credential_encrypt_failed")
    destination.parent.mkdir(mode=0o700, parents=True, exist_ok=True)
    fd, temporary = tempfile.mkstemp(prefix=".momso-credential-", dir=destination.parent)
    try:
        with os.fdopen(fd, "wb") as out:
            out.write(result.stdout)
            out.flush()
            os.fsync(out.fileno())
        try:
            os.link(temporary, destination)
        except FileExistsError:
            return "conflict"
        return "created"
    finally:
        if os.path.exists(temporary):
            os.unlink(temporary)
