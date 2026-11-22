"""Provision an IAP-protected, single-VM Cloud Run wake service."""
import re
import time
from pathlib import Path

from momso_server_install import InstallError, REGION, ZONE, save_state


class GoogleWake:
    def __init__(self, provider):
        self.p = provider
        self.service = "momso-wake-" + provider.install_id
        self.sa = "mw-" + provider.install_id
        self.email = self.sa + "@" + provider.project + ".iam.gserviceaccount.com"
        self.role = "momsoWake" + provider.install_id

    def _objects(self):
        p = self.p
        accounts = p.run("iam", "service-accounts", "list", parse=True)
        roles = p.run("iam", "roles", "list", parse=True)
        services = p.run("run", "services", "list", f"--region={REGION}", parse=True)
        account = next((x for x in accounts if x.get("email") == self.email), None)
        role = next((x for x in roles if x.get("name", "").endswith("/roles/" + self.role)), None)
        service = next((x for x in services if x.get("metadata", {}).get("name") == self.service), None)
        if account:
            account = p.run("iam", "service-accounts", "describe", self.email, parse=True)
        if role:
            role = p.run("iam", "roles", "describe", self.role, parse=True)
        if account and account.get("description") != p.marker:
            raise InstallError("깨우기 서비스 계정의 소유권이 달라 변경하지 않았습니다.")
        if role and (role.get("description") != p.marker or set(role.get("includedPermissions", [])) != {"compute.instances.get", "compute.instances.start"} or role.get("deleted")):
            raise InstallError("깨우기 권한의 소유권이나 범위가 달라 변경하지 않았습니다.")
        if service and service.get("metadata", {}).get("labels", {}).get("momso-install-id") != p.install_id:
            raise InstallError("깨우기 서비스의 소유권이 달라 변경하지 않았습니다.")
        return account, role, service

    def prepare_identity(self):
        p = self.p
        account, role, service = self._objects()
        if not account:
            p.run("iam", "service-accounts", "create", self.sa, f"--description={p.marker}")
        if not role:
            p.run("iam", "roles", "create", self.role, f"--title={self.role}", f"--description={p.marker}", "--permissions=compute.instances.get,compute.instances.start", "--stage=GA")
        for attempt in range(6):
            try:
                p.run("compute", "instances", "add-iam-policy-binding", p.name, f"--zone={ZONE}", f"--member=serviceAccount:{self.email}", f"--role=projects/{p.project}/roles/{self.role}")
                break
            except InstallError:
                if role or attempt == 5:
                    raise
                time.sleep(5)
        return service

    def install(self, hostname, user):
        p = self.p
        if not re.fullmatch(r"[^\s@]+@[^\s@]+\.[^\s@]+", user):
            raise InstallError("깨우기 로그인 이메일을 확인하세요.")
        self.prepare_identity()
        source = Path(__file__).with_name("momso_wake").resolve()
        env = f"MOMSO_PROJECT={p.project},MOMSO_ZONE={ZONE},MOMSO_INSTANCE={p.name},MOMSO_MOBILE_HOST={hostname}"
        p.run("run", "deploy", self.service, f"--region={REGION}", f"--source={source}", "--no-allow-unauthenticated", "--iap", f"--service-account={self.email}", f"--labels=momso-install-id={p.install_id}", f"--set-env-vars={env}")
        project = p.run("projects", "describe", p.project, parse=True)
        number = str(project.get("projectNumber", ""))
        if not re.fullmatch(r"[0-9]+", number):
            raise InstallError("프로젝트 번호를 확인하지 못했습니다.")
        p.run("services", "identity", "create", "--service=iap.googleapis.com")
        p.run("run", "services", "add-iam-policy-binding", self.service, f"--region={REGION}", f"--member=serviceAccount:service-{number}@gcp-sa-iap.iam.gserviceaccount.com", "--role=roles/run.invoker")
        p.run("iap", "web", "add-iam-policy-binding", f"--member=user:{user}", "--role=roles/iap.httpsResourceAccessor", f"--region={REGION}", "--resource-type=cloud-run", f"--service={self.service}")
        details = p.run("run", "services", "describe", self.service, f"--region={REGION}", parse=True)
        url = details.get("status", {}).get("url", "")
        if not re.fullmatch(r"https://[a-z0-9-]+\.[a-z0-9.-]+\.run\.app", url):
            raise InstallError("깨우기 서비스 주소를 확인하지 못했습니다.")
        return url

    def destroy(self):
        p = self.p
        account, role, service = self._objects()
        if service:
            p.run("run", "services", "delete", self.service, f"--region={REGION}")
        if account:
            p.run("iam", "service-accounts", "delete", self.email)
        if role:
            p.run("iam", "roles", "delete", self.role)


def install(provider, state_path, state, hostname, user):
    wake = state.setdefault("wake", {"hostname": hostname, "user": user})
    if wake.get("hostname") != hostname or wake.get("user") != user:
        raise InstallError("깨우기 설정이 기존 설치 기록과 다릅니다.")
    wake.pop("destroyed", None)
    save_state(state_path, state)
    wake["url"] = provider.install(hostname, user)
    save_state(state_path, state)
    return wake["url"]
