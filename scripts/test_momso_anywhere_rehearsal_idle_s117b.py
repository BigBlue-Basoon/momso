import unittest
from unittest.mock import patch

import momso_anywhere_rehearsal_idle_s117b as idle
import momso_anywhere_rehearsal_s117a as rehearsal


class Provider:
    install_id = 'a' * 12
    name = 'momso-' + install_id


class FakeClock:
    def __init__(self): self.now = 0
    def __call__(self): return self.now
    def sleep(self, seconds): self.now += seconds


class IdleRehearsalTests(unittest.TestCase):
    def test_login_prompt_flushes_even_if_output_is_piped(self):
        with patch('builtins.print') as output:
            idle._notify('login needed')
        output.assert_called_once_with('login needed', flush=True)

    def test_stops_and_owner_wakes_then_reports_ready_without_forcing_start(self):
        clock, notices, paths = FakeClock(), [], []
        states = iter(['RUNNING', 'TERMINATED', 'TERMINATED', 'RUNNING'])
        def observe(_url, path):
            paths.append(path)
            return {'status': 200, 'kind': 'application/json' if path.endswith('ready') else 'text/html'}
        operations = iter([set(), {'new-operation'}])
        result = idle.probe_idle_wake(Provider(), clock=clock, sleep=clock.sleep, notify=notices.append,
            prepare=lambda _p: None, instance=lambda _p: next(states), get_gateway=lambda _p: 'https://fixture.run.app',
            observe=observe, ready=lambda _p: True, start_operations=lambda _p: next(operations))
        self.assertEqual(result['status'], 'passed')
        self.assertEqual(result['stopSeconds'], 5)
        self.assertEqual(result['wakeSeconds'], 5)
        self.assertEqual(result['gatewayWake'], 'verified')
        self.assertEqual(result['ownerResponse'], 'unobserved')
        self.assertEqual(paths, ['/__momso/ready', '/'])
        self.assertTrue(any('직접 로그인' in notice for notice in notices))

    def test_stop_timeout_reports_gate_without_bypassing_it(self):
        clock = FakeClock()
        result = idle.probe_idle_wake(Provider(), clock=clock, sleep=clock.sleep, prepare=lambda _p: None,
            instance=lambda _p: 'RUNNING', get_gateway=lambda _p: 'https://fixture.run.app',
            diagnostics=lambda _p: {'shutdownReady': False, 'busy': False}, notify=lambda _x: None, stop_timeout=10)
        self.assertEqual(result['stage'], 'waiting_for_idle_stop')
        self.assertEqual(result['stopSeconds'], 10)
        self.assertFalse(result['activity']['shutdownReady'])

    def test_unavailable_owner_login_is_reported_and_cleanup_still_runs(self):
        clock, calls = FakeClock(), []
        states = iter(['TERMINATED'] + ['TERMINATED'] * 4)
        def check(provider):
            return idle.probe_idle_wake(provider, clock=clock, sleep=clock.sleep, prepare=lambda _p: None,
                instance=lambda _p: next(states), get_gateway=lambda _p: 'https://fixture.run.app', notify=lambda _x: None,
                observe=lambda _url, path: {'status': 200, 'kind': 'application/json' if path.endswith('ready') else 'text/html'}, start_operations=lambda _p: set(), wake_timeout=10)
        checks = dict.fromkeys(('traffic_digest', 'public_https', 'vm_services', 'update_status'), 'passed')
        result = rehearsal.run_rehearsal(Provider(), lambda: True, lambda: checks, lambda: calls.append('destroy') or True,
            lambda: calls.append('backup') or 'ok', lambda: ({}, []), idle=check)
        self.assertEqual(result['idleWake']['stage'], 'waiting_for_owner_login_and_wake')
        self.assertEqual(result['idleWake']['wakeSeconds'], 10)
        self.assertEqual(calls, ['destroy', 'backup'])

    def test_running_vm_without_gateway_operation_does_not_count_as_wake(self):
        clock = FakeClock()
        states = iter(['TERMINATED', 'RUNNING'])
        result = idle.probe_idle_wake(Provider(), clock=clock, sleep=clock.sleep, prepare=lambda _p: None,
            instance=lambda _p: next(states, 'RUNNING'), get_gateway=lambda _p: 'https://fixture.run.app', notify=lambda _x: None,
            observe=lambda _url, path: {'status': 200, 'kind': 'application/json' if path.endswith('ready') else 'text/html'},
            ready=lambda _p: True, start_operations=lambda _p: set(), wake_timeout=10)
        self.assertEqual(result['stage'], 'gateway_wake_verification')
        self.assertEqual(result['status'], 'failed')

    def test_wake_timeout_distinguishes_started_operation_from_missing_login(self):
        clock = FakeClock()
        operations = iter([set(), {'started'}])
        result = idle.probe_idle_wake(Provider(), clock=clock, sleep=clock.sleep, prepare=lambda _p: None,
            instance=lambda _p: 'TERMINATED', get_gateway=lambda _p: 'https://fixture.run.app', notify=lambda _x: None,
            observe=lambda _url, path: {'status': 200, 'kind': 'application/json' if path.endswith('ready') else 'text/html'},
            start_operations=lambda _p: next(operations), wake_timeout=10)
        self.assertEqual(result['stage'], 'waiting_for_vm_start')
        self.assertEqual(result['gatewayWake'], 'verified')
        self.assertEqual(result['status'], 'failed')

    def test_protected_id_refused_before_any_operation(self):
        provider = Provider()
        provider.install_id = 'bbbbbbbbbbbb'
        provider.protected_installation_ids = {'bbbbbbbbbbbb'}
        calls = []
        with self.assertRaises(ValueError):
            idle.probe_idle_wake(provider, prepare=lambda _p: calls.append('prepare'))
        self.assertEqual(calls, [])

    def test_only_gateway_account_starting_this_vm_counts(self):
        class OpsProvider(Provider):
            project = 'fake-project'
            def run(self, *args, **kwargs):
                target = 'https://example.test/zones/asia-northeast3-a/instances/' + self.name
                return [
                    {'name': 'approved', 'operationType': 'start', 'status': 'DONE', 'targetLink': target, 'user': 'ma-' + self.install_id + '@' + self.project + '.iam.gserviceaccount.com'},
                    {'name': 'other-user', 'operationType': 'start', 'status': 'DONE', 'targetLink': target, 'user': 'person@example.test'},
                    {'name': 'other-vm', 'operationType': 'start', 'status': 'DONE', 'targetLink': target + '-other', 'user': 'ma-' + self.install_id + '@' + self.project + '.iam.gserviceaccount.com'},
                ]
        self.assertEqual(idle.gateway_start_operations(OpsProvider()), {'approved'})

    def test_login_observation_does_not_return_cookie_or_page_content(self):
        class Response:
            status = 200
            headers = {'Content-Type': 'text/html; charset=utf-8', 'Set-Cookie': 'fixture-secret'}
            def close(self): pass
        def open_url(request, timeout):
            self.assertEqual(request.full_url, 'https://fixture.run.app/')
            self.assertEqual(timeout, 20)
            return Response()
        result = idle.public_observation('https://fixture.run.app', '/', open_url=open_url)
        self.assertEqual(result, {'status': 200, 'kind': 'text/html'})
        self.assertNotIn('fixture-secret', repr(result))

    def test_no_idle_override_after_initial_verification_fails(self):
        calls = []
        result = rehearsal.run_rehearsal(Provider(), lambda: True, lambda: {}, lambda: calls.append('destroy') or True,
            lambda: 'ok', lambda: ({}, []), idle=lambda _p: calls.append('idle'))
        self.assertEqual(result['idleWake']['stage'], 'initial_checks')
        self.assertEqual(calls, ['destroy'])


if __name__ == '__main__':
    unittest.main()
