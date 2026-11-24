"""In-process one-use pause lease; no network endpoint is exposed here."""
import hashlib
import hmac
import os
from pathlib import Path
import secrets
import threading
import time


class PauseTickets:
    def __init__(self, run_guard, *, ttl=300, clock=time.monotonic, journal=None, recover_guard=None):
        if ttl <= 0:
            raise ValueError('invalid_ttl')
        self._run_guard, self._ttl, self._clock = run_guard, ttl, clock
        self._condition = threading.Condition()
        self._state = 'idle'
        self._digest = None
        self._deadline = None
        self._reason = None
        self._reissued = False
        self._journal = Path(journal) if journal is not None else None
        if self._journal is not None and self._journal.exists():
            value = self._journal.read_text(encoding='ascii')
            if value not in ('preparing\n', 'confirmed\n') or recover_guard is None:
                raise ValueError('invalid_pause_journal')
            self._state = 'recovering'
            threading.Thread(target=self._worker, args=(recover_guard, value == 'confirmed\n'), daemon=True).start()

    def _save(self, value):
        if self._journal is None:
            return
        self._journal.parent.mkdir(parents=True, exist_ok=True, mode=0o700)
        temp = self._journal.with_name(self._journal.name + '.new')
        with open(temp, 'w', encoding='ascii') as stream:
            os.chmod(temp, 0o600)
            stream.write(value + '\n')
            stream.flush()
            os.fsync(stream.fileno())
        os.replace(temp, self._journal)
        if os.name == 'posix':
            fd = os.open(self._journal.parent, os.O_RDONLY | os.O_DIRECTORY)
            try:
                os.fsync(fd)
            finally:
                os.close(fd)

    def _clear(self):
        if self._journal is not None:
            self._journal.unlink(missing_ok=True)
            if os.name == 'posix':
                fd = os.open(self._journal.parent, os.O_RDONLY | os.O_DIRECTORY)
                try:
                    os.fsync(fd)
                finally:
                    os.close(fd)

    def _ticket(self):
        ticket = secrets.token_urlsafe(32)
        self._digest = hashlib.sha256(ticket.encode('ascii')).digest()
        return ticket

    def _matches(self, ticket):
        if not isinstance(ticket, str) or len(ticket) > 128:
            return False
        try:
            digest = hashlib.sha256(ticket.encode('ascii')).digest()
        except UnicodeEncodeError:
            return False
        return self._digest is not None and hmac.compare_digest(digest, self._digest)

    def _expire(self):
        if self._state == 'prepared' and self._clock() >= self._deadline:
            self._state = 'releasing'
            self._digest = None
            self._condition.notify_all()
        elif self._state == 'confirmed' and self._reissued and self._clock() >= self._deadline:
            self._reissued, self._digest = False, None

    def prepare(self, timeout=300):
        with self._condition:
            if self._state != 'idle':
                return {'ok': False, 'reason': 'maintenance_in_progress'}
            self._save('preparing')
            self._state, self._reason = 'preparing', None
            threading.Thread(target=self._worker, daemon=True).start()
            deadline = self._clock() + timeout
            while self._state == 'preparing':
                remaining = deadline - self._clock()
                if remaining <= 0:
                    self._state = 'releasing'
                    self._condition.notify_all()
                    return {'ok': False, 'reason': 'backup_freshness_unverified'}
                self._condition.wait(min(remaining, 1))
            if self._state != 'prepared':
                return {'ok': False, 'reason': self._reason or 'backup_freshness_unverified'}
            return {'ok': True, 'ticket': self._ticket(), 'expiresIn': self._ttl}

    def _worker(self, guard=None, confirmed_recovery=False):
        reached_hold = False
        def hold():
            nonlocal reached_hold
            with self._condition:
                if self._state == 'recovering':
                    self._state = 'confirmed' if confirmed_recovery else 'releasing'
                elif self._state == 'preparing':
                    self._state, self._deadline = 'prepared', self._clock() + self._ttl
                else:
                    return False
                reached_hold = True
                self._condition.notify_all()
                while self._state in ('prepared', 'confirmed'):
                    self._expire()
                    if self._state == 'prepared':
                        self._condition.wait(max(0, min(1, self._deadline - self._clock())))
                    elif self._state == 'confirmed':
                        self._condition.wait(1)
                return False
        try:
            result = (guard or self._run_guard)(hold)
        except Exception:
            result = {'ok': False, 'reason': 'maintenance_release_unconfirmed'}
        with self._condition:
            reason = result.get('reason') if isinstance(result, dict) else None
            if guard is not None and not reached_hold:
                reason = 'maintenance_release_unconfirmed'
            if reason not in ('maintenance_release_unconfirmed', 'maintenance_state_unknown'):
                try:
                    self._clear()
                except OSError:
                    reason = 'maintenance_release_unconfirmed'
            self._reason = reason
            self._digest, self._deadline = None, None
            self._state = 'failed' if reason in ('maintenance_release_unconfirmed', 'maintenance_state_unknown') else 'idle'
            self._condition.notify_all()

    def confirm(self, ticket):
        with self._condition:
            self._expire()
            if not (self._state == 'prepared' or (self._state == 'confirmed' and self._reissued)) or not self._matches(ticket):
                return {'ok': False, 'reason': 'invalid_pause_ticket'}
            self._save('confirmed')
            self._state = 'confirmed'
            self._reissued = False
            self._digest = None
            release_ticket = self._ticket()
            self._condition.notify_all()
            return {'ok': True, 'releaseTicket': release_ticket}

    def release(self, ticket=None, *, owner=False, timeout=30):
        with self._condition:
            self._expire()
            if self._state not in ('prepared', 'confirmed') or (not owner and not self._matches(ticket)):
                return {'ok': False, 'reason': 'invalid_pause_ticket'}
            self._state, self._digest = 'releasing', None
            self._condition.notify_all()
            deadline = self._clock() + timeout
            while self._state == 'releasing':
                remaining = deadline - self._clock()
                if remaining <= 0:
                    return {'ok': False, 'reason': 'maintenance_release_unconfirmed'}
                self._condition.wait(min(remaining, 1))
            return {'ok': self._state == 'idle', 'reason': None if self._state == 'idle' else 'maintenance_release_unconfirmed'}

    def status(self):
        with self._condition:
            self._expire()
            return {'state': self._state}

    def reissue(self):
        with self._condition:
            if self._state != 'confirmed':
                return {'ok': False, 'reason': 'invalid_pause_state'}
            self._reissued = True
            self._deadline = self._clock() + self._ttl
            return {'ok': True, 'ticket': self._ticket()}
