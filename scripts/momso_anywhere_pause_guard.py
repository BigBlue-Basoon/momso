"""Fail-closed pause workflow. Adapters must hold the VM lifecycle lock."""
import time
from contextlib import nullcontext


def pause_using_server_admission(admission, delete, *, sleep=time.sleep, attempts=120):
    """Use the updater's existing VM-local lifecycle lock and admission gate.

    The caller must run on the VM.  This does not provide a remote transport.
    """
    class Adapter:
        def activity(self):
            return admission._request('/activity')

        def acquire(self):
            if not admission.acquire():
                return {'maintenance': False}
            return {'maintenance': True, 'closedAt': admission.closed_at}

        def release(self):
            admission.release()
            return {'maintenance': False}

    result = pause_with_current_backup(Adapter(), delete, nullcontext(), sleep=sleep, attempts=attempts)
    # Lost acquire replies may leave the updater's lock held. Only this owner
    # can attempt release; if it cannot confirm release, preserve that fact.
    if admission.held and not result.get('ok'):
        try:
            admission.release()
        except Exception:
            return {'ok': False, 'reason': 'maintenance_release_unconfirmed'}
    return result


def pause_with_current_backup(server, delete, lifecycle_lock, *, sleep=time.sleep, attempts=120):
    """Delete only while admission is closed and the object backup is current.

    `delete` must return True only when the VM is confirmed removed. The lock
    adapter must be the same cross-process lifecycle lock as updater/idle stop.
    This function is not yet connected to a remote VM or a product command.
    """
    if not callable(delete) or attempts < 1:
        return {'ok': False, 'reason': 'invalid_request'}
    acquired = False
    removed = False
    result = {'ok': False, 'reason': 'pause_unavailable'}
    try:
        with lifecycle_lock:
            try:
                try:
                    before = server.activity()
                except (ConnectionError, TimeoutError, OSError):
                    return {'ok': False, 'reason': 'vm_unreachable'}
                if before.get('maintenance') is not False:
                    return {'ok': False, 'reason': 'maintenance_in_progress'}
                # An interrupted acquire can have reached the server even if
                # its response was lost. Never call deletion in that state and
                # do not release a gate whose ownership cannot be proved.
                try:
                    acquisition = server.acquire()
                except Exception:
                    return {'ok': False, 'reason': 'maintenance_state_unknown'}
                if acquisition.get('maintenance') is not True:
                    return {'ok': False, 'reason': 'maintenance_state_unknown'}
                acquired = True
                closed_at = acquisition.get('closedAt')
                if not isinstance(closed_at, int) or isinstance(closed_at, bool):
                    result = {'ok': False, 'reason': 'maintenance_time_unavailable'}
                else:
                    result = None
                for _ in range(attempts if result is None else 0):
                    view = server.activity()
                    backups = [view.get('githubBackup'), view.get('objectBackup')]
                    if any(not isinstance(backup, dict) for backup in backups):
                        result = {'ok': False, 'reason': 'backup_status_unavailable'}
                        break
                    enabled = [backup for backup in backups if backup.get('state') != 'not_configured']
                    if not enabled:
                        result = {'ok': False, 'reason': 'backup_not_connected'}
                        break
                    if any(backup.get('state') == 'failed' for backup in enabled):
                        result = {'ok': False, 'reason': 'backup_failed'}
                        break
                    if (view.get('maintenance') is True and view.get('quiescent') is True
                            and view.get('busy') is False and view.get('maintenanceReady') is True
                            and all(backup.get('state') == 'saved'
                                    and isinstance(backup.get('lastSuccessAt'), int)
                                    and not isinstance(backup.get('lastSuccessAt'), bool)
                                    and backup['lastSuccessAt'] > closed_at for backup in enabled)):
                        if delete() is True:
                            removed = True
                            result = {'ok': True, 'reason': None}
                        else:
                            result = {'ok': False, 'reason': 'delete_unconfirmed'}
                        break
                    sleep(1)
                else:
                    if result is None:
                        result = {'ok': False, 'reason': 'backup_timeout'}
            finally:
                if acquired and not removed:
                    try:
                        if server.release().get('maintenance') is not False:
                            result = {'ok': False, 'reason': 'maintenance_release_unconfirmed'}
                    except Exception:
                        result = {'ok': False, 'reason': 'maintenance_release_unconfirmed'}
    except Exception:
        if not isinstance(result, dict) or result.get('reason') != 'maintenance_release_unconfirmed':
            result = {'ok': False, 'reason': 'pause_unavailable'}
    return result
