"""Discover signed release assets without GitHub credentials."""
from __future__ import annotations

import json
import time
import urllib.error
import urllib.request
from urllib.parse import urlparse

from update import UpdateError

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
            signature = urls.get("release.json.sig") or urls.get("release.sig")
            if "release.json" in urls and signature:
                candidates.append((published, urls["release.json"], signature))
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
            if pair is None:
                self.updater._emit("up_to_date")
                return "up_to_date"
            return self.updater.install(*pair)
        except UpdateError as error:
            self.updater._emit("failed", str(error))
            raise

    def run_forever(self, stop, *, interval_seconds=3600):
        """Run independently of the server; retry recovery and polling each interval."""
        if interval_seconds <= 0:
            raise ValueError("invalid_interval")
        while not stop.is_set():
            delay = interval_seconds
            try:
                self.updater.recover()
                self.poll()
            except UpdateError as error:
                self.updater._emit("failed", str(error))
                if isinstance(error, RateLimited):
                    delay = max(delay, error.retry_seconds)
            except (OSError, ValueError):
                self.updater._emit("failed", "update_unavailable")
            if stop.wait(delay):
                break
