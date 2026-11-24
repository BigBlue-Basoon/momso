"""Small public-to-authenticated-clients update status, without local paths."""
import json
import os
from pathlib import Path
import re
import tempfile

STATES = {"checking", "up_to_date", "downloading", "waiting_for_idle", "backing_up",
          "installing", "updating_gateway", "completed", "rolling_back", "rolled_back",
          "recovery_required", "failed"}


class ProgressFile:
    def __init__(self, path):
        self.path = Path(path)

    def __call__(self, event):
        state, reason = event.get("state"), event.get("reason")
        if state not in STATES or (reason is not None and
                (not isinstance(reason, str) or not re.fullmatch(r"[a-z0-9_]{1,80}", reason))):
            raise ValueError("invalid_progress")
        detail = {}
        if 'code' in event:
            if (not isinstance(event['code'], str) or not re.fullmatch(r'gateway_[a-z0-9_]{1,72}', event['code']) or
                    any(event.get(key) is not None and type(event.get(key)) is not bool for key in ('rolledBack', 'retryable'))):
                raise ValueError('invalid_progress')
            detail = {key: event.get(key) for key in ('code', 'rolledBack', 'retryable')}
        self.path.parent.mkdir(mode=0o755, parents=True, exist_ok=True)
        fd, name = tempfile.mkstemp(prefix=".status-", dir=self.path.parent)
        try:
            with os.fdopen(fd, "w", encoding="utf-8") as stream:
                json.dump({"state": state, "reason": reason, **detail}, stream)
                stream.flush()
                os.fsync(stream.fileno())
            os.chmod(name, 0o644)
            os.replace(name, self.path)
        finally:
            if os.path.exists(name):
                os.unlink(name)
