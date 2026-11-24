"""Bind pause tickets to the VM's server admission and lifecycle lock."""
from pathlib import Path
import sys

from momso_anywhere_pause_guard import pause_using_server_admission
from momso_anywhere_pause_ticket import PauseTickets


def create_vm_pause(*, token='/run/momso-vault/server-token', lifecycle='/var/lib/momso/lifecycle',
                    updater='/opt/momso-updater', port=8765, ttl=300):
    sys.path.insert(0, str(updater))
    from server_admission import ServerAdmission

    def new_admission():
        return ServerAdmission(token, lifecycle, port=port)

    def prepare(hold):
        return pause_using_server_admission(new_admission(), hold)

    def recover(hold):
        admission = new_admission()
        try:
            if not admission.reacquire_for_recovery():
                return {'ok': False, 'reason': 'maintenance_release_unconfirmed'}
            hold()
            admission.release()
            return {'ok': False, 'reason': 'delete_unconfirmed'}
        except Exception:
            return {'ok': False, 'reason': 'maintenance_release_unconfirmed'}

    return PauseTickets(prepare, ttl=ttl, journal=Path(lifecycle) / 'pause-ticket.pending', recover_guard=recover)
