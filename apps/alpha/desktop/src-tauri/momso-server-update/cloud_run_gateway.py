"""Shared digest-pinned Cloud Run deployment adapter for manual and automatic use."""
import json
import re
import subprocess


class GatewayError(Exception):
    pass


class CloudRunGateway:
    def __init__(self, project, region, service, *, runner=subprocess.run):
        if not re.fullmatch(r"[a-z][a-z0-9-]{4,28}[a-z0-9]", project): raise ValueError("invalid_project")
        if not re.fullmatch(r"[a-z0-9-]+", region): raise ValueError("invalid_region")
        if not re.fullmatch(r"[a-z][a-z0-9-]{0,62}", service): raise ValueError("invalid_service")
        self.project, self.region, self.service, self.runner = project, region, service, runner

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

    def current(self):
        status = self._describe().get("status", {})
        entries = status.get("traffic", [])
        traffic = {}
        for item in entries:
            percent, revision = item.get("percent", 0), item.get("revisionName")
            if percent and (type(percent) is not int or not revision or not re.fullmatch(r"[a-z0-9-]+", revision)):
                raise GatewayError("gateway_response_invalid")
            if percent: traffic[revision] = traffic.get(revision, 0) + percent
        if sum(traffic.values()) != 100: raise GatewayError("gateway_response_invalid")
        return {"traffic": traffic}

    def activate(self, release, *, deploy_flags=()):
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
            self._run("deploy", self.service, "--image", release, "--no-traffic", *deploy_flags)
            revision = self._describe().get("status", {}).get("latestReadyRevisionName")
            if not revision or not re.fullmatch(r"[a-z0-9-]+", revision): raise GatewayError("gateway_not_ready")
            target = revision + "=100"
        self._run("services", "update-traffic", self.service, "--to-revisions", target)

    def healthy(self):
        status = self._describe().get("status", {})
        return any(item.get("type") == "Ready" and item.get("status") == "True" for item in status.get("conditions", []))
