"""Owner-only signed browser session; the login page is hosted at a fixed origin."""
import base64
import hashlib
import hmac
import json
import os
import secrets
import time
from pathlib import Path

SESSION_SECONDS = 30 * 24 * 60 * 60

def _key():
    value = Path(os.environ["MOMSO_SESSION_KEY_FILE"]).read_bytes().strip()
    if len(value) < 32:
        raise ValueError("session key too short")
    return value


def _b64(value):
    return base64.urlsafe_b64encode(value).rstrip(b"=").decode("ascii")


def _unb64(value):
    return base64.urlsafe_b64decode(value + "=" * (-len(value) % 4))


def _sign(kind, payload):
    value = _b64(json.dumps(payload, separators=(",", ":"), sort_keys=True).encode())
    mac = hmac.new(_key(), (kind + "." + value).encode(), hashlib.sha256).digest()
    return value + "." + _b64(mac)


def _verify(kind, value, max_age, now=None):
    try:
        encoded, signature = value.split(".")
        expected = hmac.new(_key(), (kind + "." + encoded).encode(), hashlib.sha256).digest()
        if not hmac.compare_digest(_unb64(signature), expected):
            return None
        payload = json.loads(_unb64(encoded))
        now = int(time.time() if now is None else now)
        if not isinstance(payload, dict) or not isinstance(payload.get("iat"), int) or not 0 <= now - payload["iat"] <= max_age:
            return None
        return payload
    except (ValueError, TypeError, KeyError, OSError, json.JSONDecodeError):
        return None


def challenge(origin):
    return _sign("challenge", {"iat": int(time.time()), "origin": origin, "random": secrets.token_urlsafe(32)})


def verify_google(credential, client_id):
    from google.auth.transport.requests import Request
    from google.oauth2.id_token import verify_oauth2_token
    return verify_oauth2_token(credential, Request(), client_id)


def finish(state, cookie_state, credential, origin, verifier=verify_google):
    if not state or not cookie_state or not hmac.compare_digest(state, cookie_state):
        raise ValueError("challenge mismatch")
    payload = _verify("challenge", state, 300)
    if not payload or payload.get("origin") != origin:
        raise ValueError("invalid challenge")
    claims = verifier(credential, os.environ["MOMSO_GOOGLE_CLIENT_ID"])
    expected_nonce = _b64(hashlib.sha256(state.encode()).digest())
    if claims.get("sub") != os.environ["MOMSO_OWNER_SUB"] or not hmac.compare_digest(claims.get("nonce", ""), expected_nonce):
        raise ValueError("identity mismatch")
    return _sign("session", {"iat": int(time.time()), "sub": claims["sub"]})


def valid_session(value):
    payload = _verify("session", value, SESSION_SECONDS)
    return bool(payload and payload.get("sub") == os.environ.get("MOMSO_OWNER_SUB"))
