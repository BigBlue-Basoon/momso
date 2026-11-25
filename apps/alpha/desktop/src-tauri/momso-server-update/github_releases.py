"""Discover signed release assets without GitHub credentials."""
from __future__ import annotations

import json
import base64
import binascii
import re
import time
import urllib.error
import urllib.request
from urllib.parse import urlparse

from update import UpdateError, _download
from trust_rotation import TrustError, parse_root

DEFAULT_API = "https://api.github.com/repos/BigBlue-Basoon/momso/releases"


class RateLimited(UpdateError):
    def __init__(self, retry_seconds):
        super().__init__("release_rate_limited")
        self.retry_seconds = retry_seconds


class GitHubReleases:
    def __init__(self, channel, *, api_url=DEFAULT_API, allow_http_for_tests=False):
        if channel not in ("development", "production"):
            raise ValueError("channel_invalid")
        parsed = urlparse(api_url)
        if (parsed.username or parsed.password or not parsed.hostname
                or parsed.scheme not in ({"http", "https"} if allow_http_for_tests else {"https"})):
            raise ValueError("invalid_url")
        self.channel = channel
        self.api_url = api_url
        self.allow_http = allow_http_for_tests
        self.rotation_assets = []

    def discover(self):
        """Find the newest published release, failing closed on a truncated list."""
        separator = "&" if "?" in self.api_url else "?"
        releases = []
        for page in range(1, 11):
            request = urllib.request.Request(
                self.api_url + separator + f"per_page=100&page={page}",
                headers={"Accept": "application/vnd.github+json", "User-Agent": "momso-update",
                         "X-GitHub-Api-Version": "2022-11-28"})
            try:
                with urllib.request.urlopen(request, timeout=30) as response:
                    if not self.allow_http and urlparse(response.geturl()).scheme != "https":
                        raise UpdateError("invalid_url")
                    raw = response.read(2097153)
                if len(raw) > 2097152:
                    raise UpdateError("release_list_too_large")
                batch = json.loads(raw)
            except urllib.error.HTTPError as error:
                if error.code in (403, 429):
                    try:
                        retry = max(60, int(error.headers.get("Retry-After", "0")),
                                    int(error.headers.get("X-RateLimit-Reset", "0")) - int(time.time()))
                    except (TypeError, ValueError):
                        retry = 3600
                    raise RateLimited(retry) from None
                raise UpdateError("release_discovery_unavailable") from None
            except (OSError, ValueError, urllib.error.URLError):
                raise UpdateError("release_discovery_unavailable") from None
            if not isinstance(batch, list) or len(batch) > 100:
                raise UpdateError("release_list_invalid")
            releases.extend(batch)
            if len(batch) < 100:
                break
        else:
            raise UpdateError("release_list_too_large")
        candidates = []
        rotations = {}
        for item in releases:
            if not isinstance(item, dict) or item.get("draft") is not False:
                continue
            if item.get("prerelease") is not (self.channel == "development"):
                continue
            published = item.get("published_at")
            if not isinstance(published, str) or not published:
                continue
            assets = item.get("assets")
            if not isinstance(assets, list):
                continue
            urls = {}
            for asset in assets:
                if isinstance(asset, dict) and asset.get("state") == "uploaded":
                    name, url = asset.get("name"), asset.get("browser_download_url")
                    if name in ("release.json", "release.json.sig", "release.sig") and isinstance(url, str):
                        urls[name] = url
                    match = re.fullmatch(r'trust-root-(0|[1-9][0-9]*)\.(json|sigs\.json)', name) if isinstance(name, str) else None
                    if self.channel == 'production' and match and isinstance(url, str):
                        version, kind = int(match.group(1)), match.group(2)
                        entry = rotations.setdefault(version, {})
                        previous = entry.get(kind)
                        if previous and previous[0] == published and previous[1] != url: raise UpdateError('root_assets_ambiguous')
                        if not previous or published > previous[0]: entry[kind] = (published, url)
            signature = urls.get("release.json.sig") or urls.get("release.sig")
            if "release.json" in urls and signature:
                candidates.append((published, urls["release.json"], signature))
        self.rotation_assets = sorted((version, {kind: item[1] for kind, item in assets.items()}) for version, assets in rotations.items())
        if not candidates:
            return None
        _, manifest, signature = max(candidates)
        return manifest, signature


class ReleasePoller:
    """One polling tick; host timer owns interval and retries after failures."""
    def __init__(self, source, updater):
        if source.channel != updater.channel:
            raise ValueError("channel_mismatch")
        self.source = source
        self.updater = updater

    def poll(self):
        try:
            self.updater._emit("checking")
            pair = self.source.discover()
            if self.updater.channel == 'production' and getattr(self.updater, 'trust_store', None) is not None:
                self._apply_roots()
            if pair is None:
                self.updater._emit("up_to_date")
                return "up_to_date"
            return self.updater.install(*pair)
        except UpdateError as error:
            self.updater._emit("failed", str(error))
            raise

    def _apply_roots(self):
        store = self.updater.trust_store
        try:
            current = parse_root(store.current())['version']
            for version, assets in self.source.rotation_assets:
                if version <= current: continue
                if version != current + 1 or set(assets) != {'json', 'sigs.json'}:
                    raise UpdateError('root_assets_missing')
                raw = _download(assets['json'], self.updater.allow_http, 65536)
                if parse_root(raw)['version'] != version: raise UpdateError('root_version_invalid')
                encoded = _download(assets['sigs.json'], self.updater.allow_http, 65536)
                values = json.loads(encoded.decode('utf-8'))
                if not isinstance(values, dict) or len(values) > 6: raise ValueError()
                signatures = {key: base64.b64decode(value, validate=True) for key, value in values.items()}
                result = store.apply(raw, signatures)
                current = version
        except (TrustError, ValueError, TypeError, UnicodeError, binascii.Error) as error:
            raise UpdateError(str(error) if isinstance(error, TrustError) else 'root_assets_invalid') from None

    def run_forever(self, stop, *, interval_seconds=3600):
        """Run independently of the server; retry recovery and polling each interval."""
        if interval_seconds <= 0:
            raise ValueError("invalid_interval")
        while not stop.is_set():
            delay = interval_seconds
            try:
                self.updater.recover()
                host = getattr(self.updater, "host", None)
                if getattr(host, "worker_restart_required", False):
                    host.restart_worker()
                if self.poll() == 'busy':
                    delay = min(interval_seconds, 5)
            except UpdateError as error:
                self.updater._emit("failed", str(error))
                if isinstance(error, RateLimited):
                    delay = max(delay, error.retry_seconds)
            except (OSError, ValueError):
                self.updater._emit("failed", "update_unavailable")
            try:
                host = getattr(self.updater, "host", None)
                if getattr(host, "worker_restart_required", False):
                    host.restart_worker()
                pending = getattr(host, "admission_pending", None)
                # A gate left closed by a failed attempt blocks people and the
                # idle stop; recover it soon instead of after the full interval.
                if pending is not None and pending() is True:
                    delay = min(delay, 60)
            except OSError:
                self.updater._emit("failed", "update_unavailable")
            if stop.wait(delay):
                break
