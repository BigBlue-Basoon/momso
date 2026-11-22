import unittest
from unittest.mock import Mock, patch
import subprocess

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

    def test_offline_lock_failure_restarts_services(self):
        host = SystemdHost.__new__(SystemdHost)
        host.admission = Mock()
        host.admission.reacquire_for_recovery.side_effect = AdmissionError("unavailable")
        host.admission.acquire_offline_for_recovery.return_value = False
        host.stop_server = Mock(); host.start_server = Mock()
        host.server_healthy = Mock(return_value=True)
        self.assertFalse(host.acquire_recovery_maintenance())
        host.start_server.assert_called_once_with()
        host.server_healthy.assert_called_once_with()

    def test_failed_stop_still_attempts_restart(self):
        host = SystemdHost.__new__(SystemdHost)
        host.admission = Mock()
        host.admission.reacquire_for_recovery.side_effect = AdmissionError("unavailable")
        host.stop_server = Mock(side_effect=RuntimeError("partial stop")); host.start_server = Mock(); host.server_healthy = Mock(return_value=True)
        with self.assertRaisesRegex(RuntimeError, "partial stop"): host.acquire_recovery_maintenance()
        host.start_server.assert_called_once_with()
        host.server_healthy.assert_called_once_with()

    def test_start_attempts_idle_even_if_proxy_fails(self):
        host = SystemdHost.__new__(SystemdHost)
        host.unit = "server.service"; host.proxy_unit = "proxy.service"; host.idle_unit = "idle.service"
        calls = []
        def run(command, **kwargs):
            calls.append(command)
            if command[2] == "proxy.service": raise subprocess.CalledProcessError(1, command)
            return subprocess.CompletedProcess(command, 0, b"", b"")
        with patch("systemd_host.subprocess.run", side_effect=run):
            with self.assertRaisesRegex(RuntimeError, "service_start_failed"): host.start_server()
        self.assertEqual([call[2] for call in calls], ["server.service", "proxy.service", "idle.service"])
