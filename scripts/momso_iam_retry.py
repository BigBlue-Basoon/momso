"""Retry only a custom role's known propagation failure."""
import re
import time


def role_propagation_failure(stderr):
    text = stderr.decode('utf-8', errors='replace') if isinstance(stderr, bytes) else str(stderr)
    return bool(re.search(r"(?is)role\s*\([^\r\n]*?/roles/[^\r\n)]*\)\s*does not exist in the resource.s hierarchy", text))


def bind_custom_role(provider, *args, sleep=None, attempts=12):
    if sleep is None:
        sleep = time.sleep
    for attempt in range(attempts):
        try:
            return provider.run(*args)
        except Exception as error:
            if not getattr(error, 'role_propagating', False) or attempt == attempts - 1:
                raise
            sleep(5)
