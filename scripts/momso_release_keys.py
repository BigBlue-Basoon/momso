"""Public-key identity checks for production and development release channels."""
import base64
import re
from pathlib import Path

PRODUCTION_KEY = Path(__file__).resolve().with_name('momso_release_public_key.pem')
DEVELOPMENT_KEY = Path.home() / 'Documents/dev/momso-vm-install/dev-signing/dev-public.pem'
PREFIX = bytes.fromhex('302a300506032b6570032100')


def public_der(path):
    try:
        value = Path(path).read_text(encoding='ascii')
        match = re.fullmatch(r'\s*-----BEGIN PUBLIC KEY-----\s+([A-Za-z0-9+/=\s]+)-----END PUBLIC KEY-----\s*', value)
        result = base64.b64decode(''.join(match.group(1).split()), validate=True) if match else b''
    except (OSError, UnicodeError, ValueError):
        result = b''
    if len(result) != 44 or not result.startswith(PREFIX):
        raise ValueError('public_key_invalid')
    return result


def validate(channel, path):
    supplied = public_der(path)
    if channel == 'production':
        if not PRODUCTION_KEY.is_file(): raise ValueError('production_key_unavailable')
        if supplied != public_der(PRODUCTION_KEY): raise ValueError('production_key_mismatch')
        if DEVELOPMENT_KEY.is_file() and supplied == public_der(DEVELOPMENT_KEY): raise ValueError('development_key_forbidden')
    elif channel == 'development':
        if PRODUCTION_KEY.is_file() and supplied == public_der(PRODUCTION_KEY): raise ValueError('production_key_forbidden')
    else:
        raise ValueError('channel_invalid')
    return supplied
