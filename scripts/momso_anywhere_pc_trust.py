"""Resolve production trust roots before the PC installer accepts a release."""
import base64
import binascii
import importlib.util
import json
import urllib.error
from pathlib import Path
from urllib.parse import urlsplit, urlunsplit

import momso_release_manifest as release

_source = Path(__file__).resolve().parent.parent / 'apps/alpha/desktop/src-tauri/momso-server-update/trust_rotation.py'
_spec = importlib.util.spec_from_file_location('momso_pc_trust_rotation', _source)
trust = importlib.util.module_from_spec(_spec); _spec.loader.exec_module(trust)


def _asset_url(manifest_url, name):
    url = urlsplit(manifest_url)
    prefix = url.path.rsplit('/', 1)[0] + '/'
    return urlunsplit((url.scheme, url.netloc, prefix + name, url.query, ''))


def _active_key(bootstrap, state_path, manifest_url):
    store = trust.RootStore(str(state_path) + '.trust', bootstrap)
    current = trust.parse_root(store.current())['version']
    for _ in range(128):
        version = current + 1
        try:
            raw = release.download(_asset_url(manifest_url, f'trust-root-{version}.json'), 65536)
        except urllib.error.HTTPError as error:
            if error.code == 404: break
            raise
        encoded = release.download(_asset_url(manifest_url, f'trust-root-{version}.sigs.json'), 65536)
        try:
            if trust.parse_root(raw)['version'] != version: raise ValueError()
            values = json.loads(encoded.decode('utf-8'))
            if not isinstance(values, dict) or len(values) > 6: raise ValueError()
            signatures = {key: base64.b64decode(value, validate=True) for key, value in values.items()}
            store.apply(raw, signatures)
        except (ValueError, TypeError, binascii.Error, trust.TrustError):
            raise ValueError('root_assets_invalid') from None
        current = version
    else:
        raise ValueError('root_chain_too_long')
    return trust.current_release_key(store.current())


def active_key(bootstrap, state_path, manifest_url):
    try:
        return _active_key(bootstrap, state_path, manifest_url)
    except trust.TrustError:
        raise ValueError('root_invalid') from None
