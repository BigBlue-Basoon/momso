"""Google Storage and Cloud KMS adapters for the anywhere backup primitive."""
import base64
import datetime
import json
import re
import urllib.error
import urllib.parse
import urllib.request


class GoogleBackupError(Exception):
    pass


class GoogleClient:
    def __init__(self, opener=None):
        self.opener = opener or urllib.request.urlopen

    def token(self):
        request = urllib.request.Request(
            "http://metadata.google.internal/computeMetadata/v1/instance/service-accounts/default/token",
            headers={"Metadata-Flavor": "Google"},
        )
        with self.opener(request, timeout=30) as response:
            return json.load(response)["access_token"]

    def request(self, url, *, data=None, method=None, content_type=None, allow_missing=False):
        headers = {"Authorization": "Bearer " + self.token()}
        if content_type:
            headers["Content-Type"] = content_type
        request = urllib.request.Request(url, data=data, method=method, headers=headers)
        try:
            with self.opener(request, timeout=120) as response:
                return response.read()
        except urllib.error.HTTPError as error:
            if error.code == 404 and allow_missing:
                return None
            # Never return provider response bodies, URLs or authorization data.
            raise GoogleBackupError(f"cloud_request_failed_{error.code}") from None
        except (urllib.error.URLError, TimeoutError):
            raise GoogleBackupError("cloud_unavailable") from None


class CloudKms:
    def __init__(self, client, key_resource):
        segment = r"[A-Za-z0-9._-]+"
        pattern = rf"projects/{segment}/locations/{segment}/keyRings/{segment}/cryptoKeys/{segment}"
        if not re.fullmatch(pattern, key_resource):
            raise ValueError("invalid Cloud KMS key resource")
        self.client = client
        self.url = "https://cloudkms.googleapis.com/v1/" + key_resource

    def _call(self, operation, field, value):
        body = json.dumps({field: base64.b64encode(value).decode("ascii")}).encode("ascii")
        result = self.client.request(self.url + ":" + operation, data=body, content_type="application/json")
        return base64.b64decode(json.loads(result)["ciphertext" if operation == "encrypt" else "plaintext"], validate=True)

    def wrap(self, key):
        return self._call("encrypt", "plaintext", key)

    def unwrap(self, key):
        return self._call("decrypt", "ciphertext", key)


class CloudStorage:
    def __init__(self, client, bucket):
        if not bucket or any(c not in "abcdefghijklmnopqrstuvwxyz0123456789-._" for c in bucket):
            raise ValueError("invalid backup bucket")
        self.client = client
        self.bucket = bucket

    @staticmethod
    def _name(slot):
        if not isinstance(slot, int) or not 0 <= slot < 7:
            raise ValueError("invalid backup slot")
        return f"momso-backup/slot-{slot}.enc"

    def _object(self, slot):
        name = urllib.parse.quote(self._name(slot), safe="")
        return f"https://storage.googleapis.com/storage/v1/b/{self.bucket}/o/{name}"

    def modified(self, slot):
        data = self.client.request(self._object(slot), allow_missing=True)
        if data is None:
            return None
        return datetime.datetime.fromisoformat(json.loads(data)["updated"].replace("Z", "+00:00"))

    def read(self, slot):
        return self.client.request(self._object(slot) + "?alt=media")

    def write(self, slot, data):
        query = urllib.parse.urlencode({"uploadType": "media", "name": self._name(slot)})
        url = f"https://storage.googleapis.com/upload/storage/v1/b/{self.bucket}/o?{query}"
        self.client.request(url, data=data, content_type="application/octet-stream")

    @staticmethod
    def _chunk_name(identifier, index):
        if not isinstance(identifier, str) or not re.fullmatch(r"[0-9a-f]{32}", identifier):
            raise ValueError("invalid backup chunk id")
        if not isinstance(index, int) or isinstance(index, bool) or index < 0 or index > 1000000:
            raise ValueError("invalid backup chunk number")
        return f"momso-backup/chunks/{identifier}/{index}"

    def write_chunk(self, identifier, index, data):
        name = self._chunk_name(identifier, index)
        query = urllib.parse.urlencode({"uploadType": "media", "name": name, "ifGenerationMatch": "0"})
        url = f"https://storage.googleapis.com/upload/storage/v1/b/{self.bucket}/o?{query}"
        self.client.request(url, data=data, content_type="application/octet-stream")

    def read_chunk(self, identifier, index):
        name = urllib.parse.quote(self._chunk_name(identifier, index), safe="")
        return self.client.request(f"https://storage.googleapis.com/storage/v1/b/{self.bucket}/o/{name}?alt=media")

    def chunk_matches(self, identifier, index, size, md5_hex):
        name = urllib.parse.quote(self._chunk_name(identifier, index), safe="")
        value = self.client.request(f"https://storage.googleapis.com/storage/v1/b/{self.bucket}/o/{name}", allow_missing=True)
        if value is None:
            return False
        try:
            metadata = json.loads(value)
            return int(metadata["size"]) == size and base64.b64decode(metadata["md5Hash"], validate=True).hex() == md5_hex
        except (ValueError, KeyError, TypeError):
            return False
