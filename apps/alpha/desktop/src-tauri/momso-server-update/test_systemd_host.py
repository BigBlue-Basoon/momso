import unittest
from unittest.mock import Mock

from server_admission import AdmissionError
from systemd_host import SystemdHost


class RecoveryTests(unittest.TestCase):
    def test_failed_new_server_is_stopped_before_offline_recovery_lock(self):
        host = SystemdHost.__new__(SystemdHost)
        host.admission = Mock()
        host.admission.reacquire_for_recovery.side_effect = AdmissionError("unavailable")
        host.admission.acquire_offline_for_recovery.return_value = True
        host.stop_server = Mock()
        self.assertTrue(host.acquire_recovery_maintenance())
        host.stop_server.assert_called_once_with()
        host.admission.acquire_offline_for_recovery.assert_called_once_with()
