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

    def bucket_retention(self):
        """Read policy without changing it; missing fields remain unknown."""
        try:
            value = json.loads(self.client.request(f"https://storage.googleapis.com/storage/v1/b/{self.bucket}"))
            if not isinstance(value, dict): raise ValueError()
            soft = value.get("softDeletePolicy")
            retention = None
            if soft is not None:
                if not isinstance(soft, dict): raise ValueError()
                raw = soft.get("retentionDurationSeconds")
                if not isinstance(raw, str) or not raw.isascii() or not raw.isdigit() or len(raw) > 12: raise ValueError()
                retention = int(raw)
            versioning = value.get("versioning")
            enabled = None
            if versioning is not None:
                if not isinstance(versioning, dict) or not isinstance(versioning.get("enabled"), bool): raise ValueError()
                enabled = versioning["enabled"]
            return {"softDeleteSeconds": retention, "versioningEnabled": enabled}
        except (ValueError, TypeError):
            raise GoogleBackupError("invalid_storage_metadata") from None

    @staticmethod
    def _fence_owner(owner):
        if not isinstance(owner, str) or not re.fullmatch(r"[0-9a-f]{32}", owner):
            raise ValueError("invalid backup fence owner")
        return owner

    def acquire_fence(self, owner):
        """Acquire or recover a durable fence; caller must also hold its local lock.

        This is not a lease: another machine cannot take it over after a timeout.
        All backup and restore writers must use the fence before it can protect
        production deletion; legacy writers do not satisfy that requirement.
        """
        owner = self._fence_owner(owner)
        name = "momso-backup/fence-v1"
        object_url = f"https://storage.googleapis.com/storage/v1/b/{self.bucket}/o/" + urllib.parse.quote(name, safe="")
        query = urllib.parse.urlencode({"uploadType": "media", "name": name, "ifGenerationMatch": "0"})
        upload = f"https://storage.googleapis.com/upload/storage/v1/b/{self.bucket}/o?{query}"
        try:
            metadata = self.client.request(upload, data=owner.encode("ascii"), content_type="application/octet-stream")
        except GoogleBackupError as error:
            if str(error) != "cloud_request_failed_412":
                raise
            metadata = self.client.request(object_url)
            try:
                generation = str(json.loads(metadata)["generation"])
            except (ValueError, TypeError, KeyError):
                raise GoogleBackupError("invalid_storage_metadata") from None
            if not generation.isascii() or not generation.isdigit() or len(generation) > 32:
                raise GoogleBackupError("invalid_storage_metadata")
            query = urllib.parse.urlencode({"alt": "media", "ifGenerationMatch": generation})
            if self.client.request(object_url + "?" + query) != owner.encode("ascii"):
                raise GoogleBackupError("backup_fence_busy")
            return generation
        try:
            generation = str(json.loads(metadata)["generation"])
        except (ValueError, TypeError, KeyError):
            raise GoogleBackupError("invalid_storage_metadata") from None
        if not generation.isascii() or not generation.isdigit() or len(generation) > 32:
            raise GoogleBackupError("invalid_storage_metadata")
        return generation

    def release_fence(self, owner, generation):
        """Release only the exact live generation still owned by this operation."""
        owner = self._fence_owner(owner)
        if not isinstance(generation, str) or not generation.isascii() or not generation.isdigit() or len(generation) > 32:
            raise ValueError("invalid storage generation")
        name = urllib.parse.quote("momso-backup/fence-v1", safe="")
        url = f"https://storage.googleapis.com/storage/v1/b/{self.bucket}/o/{name}"
        query = urllib.parse.urlencode({"alt": "media", "ifGenerationMatch": generation})
        if self.client.request(url + "?" + query) != owner.encode("ascii"):
            raise GoogleBackupError("backup_fence_busy")
        self.client.request(url + "?" + urllib.parse.urlencode({"ifGenerationMatch": generation}), method="DELETE")

    @staticmethod
    def _name(slot):
        if type(slot) is not int or not 0 <= slot < 8:
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

    def read_purge_ledger(self):
        name = urllib.parse.quote("momso-backup/purged-originals-v1.enc", safe="")
        url = f"https://storage.googleapis.com/storage/v1/b/{self.bucket}/o/{name}?alt=media"
        return self.client.request(url, allow_missing=True)

    def read_purge_ledger_versioned(self):
        name = urllib.parse.quote("momso-backup/purged-originals-v1.enc", safe="")
        url = f"https://storage.googleapis.com/storage/v1/b/{self.bucket}/o/{name}"
        metadata = self.client.request(url, allow_missing=True)
        if metadata is None:
            return None
        try:
            generation = str(json.loads(metadata)["generation"])
        except (ValueError, TypeError, KeyError):
            raise GoogleBackupError("invalid_storage_metadata") from None
        if not generation.isascii() or not generation.isdigit() or len(generation) > 32:
            raise GoogleBackupError("invalid_storage_metadata")
        query = urllib.parse.urlencode({"alt": "media", "ifGenerationMatch": generation})
        return generation, self.client.request(url + "?" + query)

    def write_purge_ledger_if_generation(self, generation, data):
        if not isinstance(generation, str) or not generation.isascii() or not generation.isdigit() or len(generation) > 32:
            raise ValueError("invalid storage generation")
        query = urllib.parse.urlencode({"uploadType": "media", "name": "momso-backup/purged-originals-v1.enc", "ifGenerationMatch": generation})
        url = f"https://storage.googleapis.com/upload/storage/v1/b/{self.bucket}/o?{query}"
        self.client.request(url, data=data, content_type="application/octet-stream")

    def write(self, slot, data):
        if slot == 7:
            raise ValueError('temporary slot requires conditional creation')
        query = urllib.parse.urlencode({"uploadType": "media", "name": self._name(slot)})
        url = f"https://storage.googleapis.com/upload/storage/v1/b/{self.bucket}/o?{query}"
        self.client.request(url, data=data, content_type="application/octet-stream")

    def read_versioned(self, slot):
        """Read a slot only if it remains the exact generation inspected."""
        metadata = self.client.request(self._object(slot), allow_missing=True)
        if metadata is None:
            return None
        try:
            generation = str(json.loads(metadata)["generation"])
        except (ValueError, KeyError, TypeError):
            raise GoogleBackupError("invalid_storage_metadata") from None
        if not generation.isascii() or not generation.isdigit() or len(generation) > 32:
            raise GoogleBackupError("invalid_storage_metadata")
        query = urllib.parse.urlencode({"alt": "media", "ifGenerationMatch": generation})
        return generation, self.client.request(self._object(slot) + "?" + query)

    def replace_if_generation(self, slot, generation, data):
        if slot == 7:
            raise ValueError('temporary slot cannot be replaced')
        if not isinstance(generation, str) or not generation.isascii() or not generation.isdigit() or len(generation) > 32:
            raise ValueError("invalid storage generation")
        query = urllib.parse.urlencode({"uploadType": "media", "name": self._name(slot), "ifGenerationMatch": generation})
        url = f"https://storage.googleapis.com/upload/storage/v1/b/{self.bucket}/o?{query}"
        self.client.request(url, data=data, content_type="application/octet-stream")

    def create_slot_if_absent(self, slot, data):
        """Atomically create a slot without overwriting a current object."""
        query = urllib.parse.urlencode({'uploadType': 'media', 'name': self._name(slot), 'ifGenerationMatch': '0'})
        self.client.request(f'https://storage.googleapis.com/upload/storage/v1/b/{self.bucket}/o?{query}', data=data, content_type='application/octet-stream')

    def delete_slot_if_generation(self, slot, generation):
        """Delete only the exact generation inspected by the caller."""
        if not isinstance(generation, str) or not generation.isascii() or not generation.isdigit() or not 0 < len(generation) <= 32 or generation == '0':
            raise ValueError('invalid storage generation')
        query = urllib.parse.urlencode({'ifGenerationMatch': generation})
        self.client.request(self._object(slot) + '?' + query, method='DELETE')

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
        try:
            return self.client.request(f"https://storage.googleapis.com/storage/v1/b/{self.bucket}/o/{name}?alt=media")
        except GoogleBackupError as error:
            if str(error) == 'cloud_request_failed_404':
                raise FileNotFoundError('backup_chunk_missing') from None
            raise

    def delete_current_chunk_if_unchanged(self, identifier, index):
        """Delete only the observed live generation; caller must handle retention and older versions."""
        name = urllib.parse.quote(self._chunk_name(identifier, index), safe="")
        url = f"https://storage.googleapis.com/storage/v1/b/{self.bucket}/o/{name}"
        metadata = self.client.request(url, allow_missing=True)
        if metadata is None:
            return False
        try:
            generation = str(json.loads(metadata)["generation"])
        except (ValueError, KeyError, TypeError):
            raise GoogleBackupError("invalid_storage_metadata") from None
        if not generation.isascii() or not generation.isdigit() or len(generation) > 32:
            raise GoogleBackupError("invalid_storage_metadata")
        self.client.request(url + "?" + urllib.parse.urlencode({"ifGenerationMatch": generation}), method="DELETE", allow_missing=True)
        return True

    def list_chunk_generations(self, identifier, index, *, soft_deleted=False):
        """Read all pages for one exact chunk name; never infer missing pages."""
        return self._list_generations(self._chunk_name(identifier, index), soft_deleted=soft_deleted)

    def list_slot_generations(self, slot, *, soft_deleted=False):
        return self._list_generations(self._name(slot), soft_deleted=soft_deleted)

    def _list_generations(self, name, *, soft_deleted):
        token = None
        seen = set()
        result = []
        for _ in range(100):
            query = {"prefix": name, "maxResults": "1000"}
            query["softDeleted" if soft_deleted else "versions"] = "true"
            if token is not None:
                query["pageToken"] = token
            url = f"https://storage.googleapis.com/storage/v1/b/{self.bucket}/o?" + urllib.parse.urlencode(query)
            try:
                page = json.loads(self.client.request(url))
                if not isinstance(page, dict): raise ValueError()
                items = page.get("items", [])
                if not isinstance(items, list): raise ValueError()
                for item in items:
                    if not isinstance(item, dict) or not isinstance(item.get("name"), str): raise ValueError()
                    if item["name"] != name: continue
                    generation = str(item["generation"])
                    if not generation.isascii() or not generation.isdigit() or len(generation) > 32: raise ValueError()
                    if any(item.get(field) is not None and (not isinstance(item[field], str) or len(item[field]) > 80) for field in ("softDeleteTime", "hardDeleteTime")): raise ValueError()
                    result.append({"generation": generation, "softDeleteTime": item.get("softDeleteTime"), "hardDeleteTime": item.get("hardDeleteTime")})
                token = page.get("nextPageToken")
                if token is None: return result
                if not isinstance(token, str) or not token or len(token) > 4096 or token in seen: raise ValueError()
                seen.add(token)
            except (ValueError, KeyError, TypeError):
                raise GoogleBackupError("invalid_storage_metadata") from None
        raise GoogleBackupError("storage_listing_limit")

    def delete_chunk_generation(self, identifier, index, generation):
        if not isinstance(generation, str) or not generation.isascii() or not generation.isdigit() or len(generation) > 32:
            raise ValueError("invalid storage generation")
        name = urllib.parse.quote(self._chunk_name(identifier, index), safe="")
        url = f"https://storage.googleapis.com/storage/v1/b/{self.bucket}/o/{name}"
        query = urllib.parse.urlencode({"generation": generation, "ifGenerationMatch": generation})
        self.client.request(url + "?" + query, method="DELETE", allow_missing=True)

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
