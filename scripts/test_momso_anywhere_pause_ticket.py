import threading
import time
import tempfile
import unittest
from pathlib import Path

from momso_anywhere_pause_ticket import PauseTickets


class PauseTicketTests(unittest.TestCase):
    def setUp(self):
        self.released = threading.Event()
        def guard(hold):
            hold()
            self.released.set()
            return {'ok': False, 'reason': 'delete_unconfirmed'}
        self.tickets = PauseTickets(guard, ttl=0.1)

    def test_prepared_ticket_expires_and_releases(self):
        ticket = self.tickets.prepare()['ticket']
        self.assertTrue(self.released.wait(2))
        self.assertFalse(self.tickets.confirm(ticket)['ok'])

    def test_confirm_consumes_ticket_and_stays_closed_until_release(self):
        ticket = self.tickets.prepare()['ticket']
        confirmed = self.tickets.confirm(ticket)
        self.assertTrue(confirmed['ok'])
        self.assertFalse(self.tickets.confirm(ticket)['ok'])
        time.sleep(0.2)
        self.assertEqual(self.tickets.status()['state'], 'confirmed')
        self.assertTrue(self.tickets.release(confirmed['releaseTicket'])['ok'])
        self.assertTrue(self.released.is_set())

    def test_owner_can_recover_and_release_after_lost_tab(self):
        ticket = self.tickets.prepare()['ticket']
        self.assertTrue(self.tickets.confirm(ticket)['ok'])
        self.assertEqual(self.tickets.status()['state'], 'confirmed')
        self.assertTrue(self.tickets.release(owner=True)['ok'])

    def test_owner_can_reissue_one_use_ticket_after_lost_tab(self):
        first = self.tickets.prepare()['ticket']
        old_release = self.tickets.confirm(first)['releaseTicket']
        replacement = self.tickets.reissue()['ticket']
        self.assertFalse(self.tickets.release(old_release)['ok'])
        confirmed = self.tickets.confirm(replacement)
        self.assertTrue(confirmed['ok'])
        self.assertFalse(self.tickets.confirm(replacement)['ok'])
        self.assertTrue(self.tickets.release(confirmed['releaseTicket'])['ok'])

    def test_expired_reissued_ticket_does_not_open_confirmed_gate(self):
        first = self.tickets.prepare()['ticket']
        self.tickets.confirm(first)
        replacement = self.tickets.reissue()['ticket']
        time.sleep(0.2)
        self.assertFalse(self.tickets.confirm(replacement)['ok'])
        self.assertEqual(self.tickets.status()['state'], 'confirmed')
        self.assertTrue(self.tickets.release(owner=True)['ok'])

    def test_backup_failure_does_not_issue_ticket(self):
        tickets = PauseTickets(lambda _: {'ok': False, 'reason': 'backup_not_connected'})
        self.assertEqual(tickets.prepare(), {'ok': False, 'reason': 'backup_not_connected'})

    def test_expiration_wins_when_time_has_reached_deadline(self):
        now = [1.0]
        tickets = PauseTickets(lambda hold: (hold(), {'ok': False, 'reason': 'delete_unconfirmed'})[1], clock=lambda: now[0])
        ticket = tickets.prepare()['ticket']
        now[0] += 300
        self.assertFalse(tickets.confirm(ticket)['ok'])

    def test_failed_recovery_keeps_confirmed_journal(self):
        with tempfile.TemporaryDirectory() as folder:
            marker = Path(folder) / 'pause.pending'
            marker.write_text('confirmed\n', encoding='ascii')
            tickets = PauseTickets(lambda _: {}, journal=marker, recover_guard=lambda _: {'ok': False, 'reason': 'unavailable'})
            deadline = time.monotonic() + 2
            while tickets.status()['state'] == 'recovering' and time.monotonic() < deadline:
                time.sleep(0.01)
            self.assertEqual(tickets.status()['state'], 'failed')
            self.assertEqual(marker.read_text(encoding='ascii'), 'confirmed\n')


if __name__ == '__main__':
    unittest.main()
