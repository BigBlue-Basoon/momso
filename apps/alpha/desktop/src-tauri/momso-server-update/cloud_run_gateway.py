"""Shared digest-pinned Cloud Run deployment adapter for manual and automatic use."""
import json
import re
import subprocess
import time
import urllib.parse
import urllib.request


class GatewayError(Exception):
    pass


def gateway_failure(code, rolled_back=None):
    aliases = {'gateway_unhealthy': 'gateway_probe_failed', 'gateway_health_failed': 'gateway_probe_failed',
               'gateway_traffic_unknown': 'gateway_traffic_mismatch', 'gateway_rollback_mismatch': 'gateway_rollback_failed',
               'gateway_restore_failed': 'gateway_rollback_failed', 'gateway_address_changed': 'gateway_address_invalid',
               'gateway_address': 'gateway_address_invalid'}
    code = aliases.get(code, code)
    retryable = {'gateway_image_permission': False, 'gateway_image_invalid': False, 'gateway_image_mismatch': False,
                 'gateway_traffic_mismatch': True, 'gateway_probe_failed': True, 'gateway_rollback_failed': False,
                 'gateway_not_ready': True, 'gateway_operation_failed': True, 'gateway_response_invalid': True,
                 'gateway_address_invalid': False, 'gateway_ownership': False, 'gateway_policy': False}.get(code)
    if code not in {'gateway_image_permission', 'gateway_image_invalid', 'gateway_image_mismatch', 'gateway_traffic_mismatch',
                    'gateway_probe_failed', 'gateway_rollback_failed', 'gateway_not_ready', 'gateway_operation_failed',
                    'gateway_response_invalid', 'gateway_address_invalid', 'gateway_ownership', 'gateway_policy'}:
        code = 'gateway_operation_failed'
        retryable = None
    if rolled_back is False:
        retryable = False
    elif rolled_back is None:
        retryable = None
    return {'code': code, 'rolledBack': rolled_back, 'retryable': retryable}


class CloudRunGateway:
    def __init__(self, project, region, service, *, runner=subprocess.run, probe=None):
        if not re.fullmatch(r"[a-z][a-z0-9-]{4,28}[a-z0-9]", project): raise ValueError("invalid_project")
        if not re.fullmatch(r"[a-z0-9-]+", region): raise ValueError("invalid_region")
        if not re.fullmatch(r"[a-z][a-z0-9-]{0,62}", service): raise ValueError("invalid_service")
        self.project, self.region, self.service, self.runner = project, region, service, runner
        self.probe = probe or self._probe

    @staticmethod
    def _probe(url):
        parsed = urllib.parse.urlsplit(url)
        if parsed.scheme != 'https' or not parsed.hostname or not parsed.hostname.endswith('.run.app') or parsed.netloc != parsed.hostname or parsed.path or parsed.query or parsed.fragment:
            raise GatewayError('gateway_address_invalid')
        class NoRedirect(urllib.request.HTTPRedirectHandler):
            def redirect_request(self, *args, **kwargs): return None
        opener = urllib.request.build_opener(NoRedirect())
        request = urllib.request.Request(url + '/__momso/ready', headers={'Cache-Control': 'no-store'})
        for attempt in range(3):
            try:
                with opener.open(request, timeout=20) as response:
                    if response.status == 200 and json.loads(response.read(256)) == {'ready': True}:
                        return
            except Exception:
                pass
            if attempt < 2: time.sleep(2)
        raise GatewayError('gateway_probe_failed')

    def _run(self, *args):
        command = ["gcloud", "run", *args, "--project", self.project, "--region", self.region, "--quiet"]
        result = self.runner(command, capture_output=True, timeout=600, check=False)
        if result.returncode:
            error = (result.stderr or b"")
            if isinstance(error, bytes): error = error.decode("utf-8", "replace")
            if "artifactregistry.repositories.downloadArtifacts" in error or "permission to read the image" in error.lower():
                raise GatewayError("gateway_image_permission")
            raise GatewayError("gateway_operation_failed")
        return result.stdout

    def preflight(self, image):
        if not isinstance(image, str) or not re.fullmatch(r"[a-zA-Z0-9._/-]+@sha256:[a-f0-9]{64}", image):
            raise GatewayError("gateway_image_invalid")
        # Artifact Registry's describe command cannot inspect public GHCR.
        # Cloud Run validates availability while staging the pinned revision.
        if image.startswith("ghcr.io/"):
            return False
        try:
            result = self.runner(["gcloud", "artifacts", "docker", "images", "describe", image, "--format=json", "--quiet"], capture_output=True, timeout=60, check=False)
        except (OSError, subprocess.TimeoutExpired):
            return False
        if result.returncode:
            error = result.stderr or b""
            if isinstance(error, bytes): error = error.decode("utf-8", "replace")
            if "artifactregistry.repositories.downloadArtifacts" in error:
                raise GatewayError("gateway_image_permission")
            # Describing an image can require a different permission than
            # pulling it; an inconclusive probe must not block an update.
            return False
        return True

    def _describe(self):
        try: return json.loads(self._run("services", "describe", self.service, "--format=json"))
        except (ValueError, TypeError): raise GatewayError("gateway_response_invalid") from None

    def _verify_revision(self, revision, image):
        try:
            data = json.loads(self._run("revisions", "describe", revision, "--format=json"))
            status = data["status"]
            ready = any(item.get("type") == "Ready" and item.get("status") == "True" for item in status.get("conditions", []))
            resolved = status.get("imageDigest", "")
            expected = image.rsplit("@", 1)[1]
            if not ready or not isinstance(resolved, str) or resolved.split("@")[-1] != expected:
                raise GatewayError("gateway_image_mismatch")
        except (ValueError, TypeError, KeyError):
            raise GatewayError("gateway_response_invalid") from None

    def current(self):
        status = self._describe().get("status", {})
        entries = status.get("traffic", [])
        traffic = {}
        for item in entries:
            percent, revision = item.get("percent", 0), item.get("revisionName")
            if percent and (type(percent) is not int or not revision or not re.fullmatch(r"[a-z0-9-]+", revision)):
                raise GatewayError("gateway_traffic_mismatch")
            if percent: traffic[revision] = traffic.get(revision, 0) + percent
        if sum(traffic.values()) != 100: raise GatewayError("gateway_traffic_mismatch")
        return {"traffic": traffic}

    def activate(self, release, *, deploy_flags=(), first_deploy=False, probe=True):
        if isinstance(release, dict):
            if deploy_flags: raise GatewayError("gateway_response_invalid")
            traffic = release.get("traffic", {})
            if not traffic or sum(traffic.values()) != 100 or any(not re.fullmatch(r"[a-z0-9-]+", name) or type(pct) is not int or pct < 0 for name, pct in traffic.items()):
                raise GatewayError("gateway_response_invalid")
            target = ",".join(f"{name}={percent}" for name, percent in sorted(traffic.items()))
        else:
            if not isinstance(release, str) or not re.fullmatch(r"[a-zA-Z0-9._/-]+@sha256:[a-f0-9]{64}", release):
                raise GatewayError("gateway_image_invalid")
            if any(not isinstance(flag, str) or not flag.startswith("--") or flag.startswith(("--source", "--image", "--traffic", "--no-traffic")) for flag in deploy_flags):
                raise GatewayError("gateway_response_invalid")
            before = None if first_deploy else self._describe().get('status', {}).get('latestCreatedRevisionName')
            if not first_deploy and (not before or not re.fullmatch(r'[a-z0-9-]+', before)): raise GatewayError('gateway_not_ready')
            traffic_flags = () if first_deploy else ("--no-traffic",)
            # The service-level cap also covers revisions during traffic switches.
            self._run("deploy", self.service, "--image", release, "--max=1", *traffic_flags, *deploy_flags)
            revision = self._describe().get("status", {}).get("latestCreatedRevisionName")
            if not revision or not re.fullmatch(r"[a-z0-9-]+", revision) or revision == before: raise GatewayError("gateway_not_ready")
            self._verify_revision(revision, release)
            target = revision + "=100"
        self._run("services", "update-traffic", self.service, "--to-revisions", target)
        if isinstance(release, dict):
            if self.current()["traffic"] != traffic:
                raise GatewayError("gateway_rollback_mismatch")
        else:
            if self.current()["traffic"] != {revision: 100}:
                raise GatewayError("gateway_traffic_mismatch")
            self._verify_revision(revision, release)
            if probe:
                self.probe(self._describe().get('status', {}).get('url', ''))

    def healthy(self):
        status = self._describe().get("status", {})
        return any(item.get("type") == "Ready" and item.get("status") == "True" for item in status.get("conditions", []))
