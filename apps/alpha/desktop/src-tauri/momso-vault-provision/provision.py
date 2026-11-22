"""Prepare the Linux vault credential without writing key material to persistent disk."""
import base64
import json
import os
from pathlib import Path
import re
import subprocess
import tempfile
import urllib.error
import urllib.request


class VaultProvisionError(Exception): pass


class NoRedirect(urllib.request.HTTPRedirectHandler):
    def redirect_request(self, req, fp, code, msg, headers, newurl): return None


def request(url, headers, data=None):
    opener = urllib.request.build_opener(urllib.request.ProxyHandler({}), NoRedirect())
    req = urllib.request.Request(url, headers=headers, data=data, method="POST" if data is not None else "GET")
    with opener.open(req, timeout=15) as response:
        return response.read(65536)


class SecretManager:
    def __init__(self, project, secret):
        if not re.fullmatch(r"[a-z][a-z0-9-]{4,28}[a-z0-9]", project) or not re.fullmatch(r"[A-Za-z0-9_-]{1,255}", secret):
            raise VaultProvisionError("configuration_invalid")
        self.base = f"https://secretmanager.googleapis.com/v1/projects/{project}/secrets/{secret}"

    def _headers(self):
        raw = request("http://metadata.google.internal/computeMetadata/v1/instance/service-accounts/default/token", {"Metadata-Flavor": "Google"})
        token = json.loads(raw)["access_token"]
        return {"Authorization": "Bearer " + token, "Content-Type": "application/json"}

    def get(self):
        try:
            result = json.loads(request(self.base + "/versions/1:access", self._headers()))
        except urllib.error.HTTPError as error:
            if error.code == 404: return None
            raise VaultProvisionError("credential_unavailable") from None
        return base64.b64decode(result["payload"]["data"], validate=True)

    def add(self, value):
        payload = json.dumps({"payload": {"data": base64.b64encode(value).decode("ascii")}}).encode()
        request(self.base + ":addVersion", self._headers(), payload)


def valid_identity(value):
    if not re.fullmatch(rb"AGE-SECRET-KEY-1[A-Z0-9]{50,100}\s*", value): return False
    checked = subprocess.run(["age-keygen", "-y"], input=value, stdout=subprocess.DEVNULL, stderr=subprocess.DEVNULL, check=False, timeout=10)
    return checked.returncode == 0


def prepare(store, destination, vault_dir, *, create=False, generator=None):
    """Caller serializes initialization across VMs; destination must be private tmpfs."""
    identity = store.get()
    if identity is None:
        if not create or any(Path(vault_dir).rglob("*.age")):
            raise VaultProvisionError("credential_missing")
        if generator is None:
            result = subprocess.run(["age-keygen"], stdout=subprocess.PIPE, stderr=subprocess.DEVNULL, check=True, timeout=10)
            output = result.stdout
        else:
            output = generator()
        lines = [line.strip() for line in output.splitlines() if line.strip() and not line.startswith(b"#")]
        if len(lines) != 1: raise VaultProvisionError("credential_invalid")
        identity = lines[0]
        if not valid_identity(identity): raise VaultProvisionError("credential_invalid")
        # Never create a second version after an ambiguous write; retry starts with get().
        store.add(identity)
        fetched = store.get()
        if fetched != identity: raise VaultProvisionError("credential_unconfirmed")
    if not valid_identity(identity): raise VaultProvisionError("credential_invalid")
    dest = Path(destination)
    dest.parent.mkdir(mode=0o700, parents=True, exist_ok=True)
    os.chmod(dest.parent, 0o700)
    fd, temp_name = tempfile.mkstemp(prefix=dest.name + ".", dir=dest.parent)
    temp = Path(temp_name)
    try:
        with os.fdopen(fd, "wb") as out: out.write(identity.strip() + b"\n")
        os.replace(temp, dest)
    finally:
        temp.unlink(missing_ok=True)


def main():
    import argparse
    parser = argparse.ArgumentParser()
    parser.add_argument("--project", required=True)
    parser.add_argument("--secret", required=True)
    parser.add_argument("--vault-dir", required=True)
    parser.add_argument("--create-initial", action="store_true")
    args = parser.parse_args()
    try:
        prepare(SecretManager(args.project, args.secret), "/run/momso-vault/momso-vault-identity", args.vault_dir, create=args.create_initial)
    except Exception:
        raise SystemExit("credential_setup_failed") from None


if __name__ == "__main__": main()
