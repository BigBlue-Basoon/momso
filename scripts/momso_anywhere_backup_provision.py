"""Provisioning helper, not yet connected to the anywhere installer."""
import re

from momso_server_install import InstallError, REGION


class GoogleBackupProvisioner:
    def __init__(self, provider, vm_email=None):
        self.p = provider
        if not re.fullmatch(r"[a-z0-9]{6,24}", provider.install_id):
            raise InstallError("백업 설치 식별자를 확인하지 못했습니다.")
        self.bucket = "momso-backup-" + provider.install_id
        self.ring = "momso-backup-" + provider.install_id
        self.key = "backup"
        self.sa = "mb-" + provider.install_id
        self.email = vm_email or self.sa + "@" + provider.project + ".iam.gserviceaccount.com"
        if vm_email and vm_email != "mv-" + provider.install_id + "@" + provider.project + ".iam.gserviceaccount.com":
            raise InstallError("VM 백업 서비스 계정이 설치 기록과 다릅니다.")
        self.existing_vm_account = vm_email is not None
        self.role = "momsoBackup" + provider.install_id

    def provision(self):
        p = self.p
        # A colliding resource must not be adopted without the install marker.
        buckets = p.run("storage", "buckets", "list", parse=True)
        found = next((x for x in buckets if x.get("name") == self.bucket), None)
        if found:
            actual = p.run("storage", "buckets", "describe", "gs://" + self.bucket, parse=True)
            iam = actual.get("iam_config", actual.get("iamConfiguration", {}))
            labels = actual.get("labels", {})
            public = iam.get("public_access_prevention", iam.get("publicAccessPrevention"))
            uniform = iam.get("uniform_bucket_level_access", iam.get("uniformBucketLevelAccess", {}))
            if labels.get("momso-install-id") != p.install_id or public != "enforced" or not uniform.get("enabled"):
                raise InstallError("백업 버킷의 소유권이나 비공개 설정이 달라 변경하지 않았습니다.")
        else:
            p.run("storage", "buckets", "create", "gs://" + self.bucket,
                  f"--location={REGION}", "--uniform-bucket-level-access",
                  "--public-access-prevention", f"--labels=momso-install-id={p.install_id}")
        accounts = p.run("iam", "service-accounts", "list", parse=True)
        account = next((x for x in accounts if x.get("email") == self.email), None)
        if self.existing_vm_account:
            if not account or p.run("iam", "service-accounts", "describe", self.email, parse=True).get("description") != p.marker:
                raise InstallError("VM 백업 서비스 계정의 소유권이 다릅니다.")
            account = {"description": p.marker}
        if account and account.get("description") != p.marker:
            raise InstallError("백업 서비스 계정의 소유권이 달라 변경하지 않았습니다.")
        if not account:
            p.run("iam", "service-accounts", "create", self.sa, f"--description={p.marker}")
        rings = p.run("kms", "keyrings", "list", f"--location={REGION}", parse=True)
        ring_resource = f"projects/{p.project}/locations/{REGION}/keyRings/{self.ring}"
        ring = next((x for x in rings if x.get("name") == ring_resource), None)
        if not ring:
            p.run("kms", "keyrings", "create", self.ring, f"--location={REGION}")
        keys = p.run("kms", "keys", "list", f"--location={REGION}", f"--keyring={self.ring}", parse=True)
        key_resource = ring_resource + "/cryptoKeys/" + self.key
        key = next((x for x in keys if x.get("name") == key_resource), None)
        if key and key.get("labels", {}).get("momso-install-id") != p.install_id:
            raise InstallError("백업 암호화 키의 소유권이 달라 변경하지 않았습니다.")
        if not key:
            # A pre-existing empty key ring cannot prove ownership.
            if ring:
                raise InstallError("기존 키 보관함의 소유권을 확인하지 못했습니다.")
            p.run("kms", "keys", "create", self.key, f"--location={REGION}",
                  f"--keyring={self.ring}", "--purpose=encryption",
                  f"--labels=momso-install-id={p.install_id}")
        permissions = ["storage.objects.get", "storage.objects.create", "storage.objects.delete"]
        roles = p.run("iam", "roles", "list", parse=True)
        role_name = f"projects/{p.project}/roles/{self.role}"
        role = next((x for x in roles if x.get("name") == role_name), None)
        if role:
            role = p.run("iam", "roles", "describe", self.role, parse=True)
            if role.get("description") != p.marker or set(role.get("includedPermissions", [])) != set(permissions) or role.get("deleted"):
                raise InstallError("백업 권한의 소유권이나 범위가 달라 변경하지 않았습니다.")
        else:
            p.run("iam", "roles", "create", self.role, f"--title={self.role}",
                  f"--description={p.marker}", "--permissions=" + ",".join(permissions), "--stage=GA")
        member = "serviceAccount:" + self.email
        p.run("storage", "buckets", "add-iam-policy-binding", "gs://" + self.bucket,
              f"--member={member}", f"--role={role_name}")
        p.run("kms", "keys", "add-iam-policy-binding", self.key,
              f"--location={REGION}", f"--keyring={self.ring}",
              f"--member={member}", "--role=roles/cloudkms.cryptoKeyEncrypterDecrypter")
        return {"bucket": self.bucket, "key": key_resource, "service_account": self.email,
                "bucket_permissions": permissions}

    def destroy(self, *, delete_backup=False):
        """Keep backups by default; destructive use requires an explicit choice."""
        if not delete_backup:
            return "backup_preserved"
        p = self.p
        buckets = p.run("storage", "buckets", "list", parse=True)
        found = next((x for x in buckets if x.get("name") == self.bucket), None)
        if not found:
            return "backup_absent"
        actual = p.run("storage", "buckets", "describe", "gs://" + self.bucket, parse=True)
        if actual.get("labels", {}).get("momso-install-id") != p.install_id:
            raise InstallError("백업 버킷의 소유권이 달라 삭제하지 않았습니다.")
        p.run("storage", "rm", "--recursive", "gs://" + self.bucket)
        # KMS keys cannot be assumed disposable merely because one bucket was
        # removed. Their lifecycle stays in the customer's project.
        return "backup_deleted"
