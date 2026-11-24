import unittest
from contextlib import nullcontext

from momso_anywhere_pause_guard import pause_using_server_admission, pause_with_current_backup


class Server:
    def __init__(self, states):
        self.states = iter(states)
        self.released = 0
    def activity(self): return next(self.states)
    def acquire(self): return {'maintenance': True, 'closedAt': 100}
    def release(self): self.released += 1; return {'maintenance': False}


def state(backup, ready=False):
    return {'maintenance': True, 'quiescent': True, 'busy': False,
            'maintenanceReady': ready, 'githubBackup': {'state': 'not_configured'},
            'objectBackup': {'state': backup, 'lastSuccessAt': 200}}


def run(server, delete, **kwargs):
    return pause_with_current_backup(server, delete, nullcontext(), sleep=lambda _: None, **kwargs)


class PauseGuardTests(unittest.TestCase):
    def test_vm_local_admission_keeps_lock_held_through_delete(self):
        class Admission:
            held = False
            closed_at = 100
            def _request(self, path):
                self_path = path
                assert self_path == '/activity'
                return state('saved', True) if self.held else {'maintenance': False}
            def acquire(self): self.held = True; return True
            def release(self): self.held = False
        admission = Admission()
        def delete():
            self.assertTrue(admission.held)
            return True
        self.assertTrue(pause_using_server_admission(admission, delete, sleep=lambda _: None)['ok'])
        self.assertTrue(admission.held)

    def test_vm_local_lost_acquire_response_releases_own_admission(self):
        class Admission:
            held = False
            closed_at = None
            def _request(self, _path): return {'maintenance': False}
            def acquire(self): self.held = True; raise OSError('lost response')
            def release(self): self.held = False
        admission = Admission()
        result = pause_using_server_admission(admission, lambda: self.fail('must not delete'), sleep=lambda _: None)
        self.assertEqual(result['reason'], 'maintenance_state_unknown')
        self.assertFalse(admission.held)

    def test_vm_local_backup_failure_reopens_gate(self):
        class Admission:
            held = False
            closed_at = 100
            def _request(self, _path): return state('failed') if self.held else {'maintenance': False}
            def acquire(self): self.held = True; return True
            def release(self): self.held = False
        admission = Admission()
        result = pause_using_server_admission(admission, lambda: self.fail('must not delete'), sleep=lambda _: None)
        self.assertEqual(result['reason'], 'backup_failed')
        self.assertFalse(admission.held)

    def test_waits_for_current_backup_and_deletes_inside_gate(self):
        server = Server([{'maintenance': False}, state('syncing'), state('saved', True)])
        calls = []
        result = run(server, lambda: calls.append('delete') or True)
        self.assertEqual(result, {'ok': True, 'reason': None})
        self.assertEqual(calls, ['delete'])
        self.assertEqual(server.released, 0)

    def test_failed_or_missing_backup_never_deletes(self):
        for status, reason in [('failed', 'backup_failed'), ('not_configured', 'backup_not_connected')]:
            server, calls = Server([{'maintenance': False}, state(status)]), []
            result = run(server, lambda: calls.append('delete'))
            self.assertEqual(result['reason'], reason)
            self.assertEqual(calls, [])
            self.assertEqual(server.released, 1)

    def test_busy_or_unverified_never_deletes(self):
        server, calls = Server([{'maintenance': False}, state('saved', False)]), []
        result = run(server, lambda: calls.append('delete'), attempts=1)
        self.assertEqual(result['reason'], 'backup_timeout')
        self.assertEqual(calls, [])
        self.assertEqual(server.released, 1)

    def test_delete_failure_releases_gate(self):
        server = Server([{'maintenance': False}, state('saved', True)])
        result = run(server, lambda: False)
        self.assertEqual(result['reason'], 'delete_unconfirmed')
        self.assertEqual(server.released, 1)

    def test_unconfirmed_gate_release_is_never_reported_as_safe(self):
        class BrokenRelease(Server):
            def release(self): raise RuntimeError('fixture unavailable')
        server = BrokenRelease([{'maintenance': False}, state('failed')])
        result = run(server, lambda: self.fail('must not delete'))
        self.assertEqual(result['reason'], 'maintenance_release_unconfirmed')

    def test_interrupted_acquisition_never_deletes_or_releases_unknown_gate(self):
        class Interrupted(Server):
            def acquire(self): raise ConnectionError('response lost')
        server = Interrupted([{'maintenance': False}])
        result = run(server, lambda: self.fail('must not delete'))
        self.assertEqual(result['reason'], 'maintenance_state_unknown')
        self.assertEqual(server.released, 0)

    def test_old_server_without_closure_time_is_released_and_not_deleted(self):
        class OldServer(Server):
            def acquire(self): return {'maintenance': True}
        server = OldServer([{'maintenance': False}])
        result = run(server, lambda: self.fail('must not delete'))
        self.assertEqual(result['reason'], 'maintenance_time_unavailable')
        self.assertEqual(server.released, 1)

    def test_existing_maintenance_is_not_taken_over(self):
        server = Server([{'maintenance': True}])
        result = run(server, lambda: self.fail('must not delete'))
        self.assertEqual(result['reason'], 'maintenance_in_progress')
        self.assertEqual(server.released, 0)

    def test_unreachable_vm_never_deletes(self):
        class Offline(Server):
            def activity(self): raise ConnectionError('offline')
        server = Offline([])
        result = run(server, lambda: self.fail('must not delete'))
        self.assertEqual(result['reason'], 'vm_unreachable')
        self.assertEqual(server.released, 0)

    def test_connection_loss_after_gate_releases_without_deleting(self):
        class Interrupted(Server):
            def activity(self):
                try: return next(self.states)
                except StopIteration: raise ConnectionError('offline')
        server = Interrupted([{'maintenance': False}])
        result = run(server, lambda: self.fail('must not delete'))
        self.assertEqual(result['reason'], 'pause_unavailable')
        self.assertEqual(server.released, 1)

    def test_old_backup_and_one_stale_branch_cannot_authorize_deletion(self):
        for github in ({'state': 'not_configured'}, {'state': 'saved', 'lastSuccessAt': 99}):
            snapshot = state('saved', True)
            snapshot['githubBackup'] = github
            if github['state'] == 'not_configured': snapshot['objectBackup']['lastSuccessAt'] = 100
            server = Server([{'maintenance': False}, snapshot])
            result = run(server, lambda: self.fail('must not delete'), attempts=1)
            self.assertEqual(result['reason'], 'backup_timeout')
            self.assertEqual(server.released, 1)


if __name__ == '__main__': unittest.main()
