import json
from pathlib import Path
import tempfile
import unittest
from unittest.mock import patch

import momso_anywhere_resize as resize


class Provider:
    name = 'momso-0123456789ab'
    def __init__(self, failure=None):
        self.calls = []
        self.failure = failure
        self.failed = False
    def run(self, *args, **kwargs):
        action = args[2]
        self.calls.append(action)
        if action == self.failure and not self.failed:
            self.failed = True
            raise RuntimeError('fixture failure')
        return {}


class ResizeTests(unittest.TestCase):
    def setUp(self):
        self.provider = Provider()
        self.gates = []
        self.owned = patch.object(resize, 'verify_owned', return_value=('e2-small', 'https://fixture.example'))
        self.owned.start()
        self.addCleanup(self.owned.stop)

    def gate(self, provider, action):
        self.gates.append(action)

    def test_success_waits_for_gateway_before_releasing_gate(self):
        probes = iter([True, False, True])
        result = resize.resize(self.provider, {}, 'e2-medium', lambda _: next(probes), self.gate, lambda _: None)
        self.assertEqual(result, {'ok': True, 'machineType': 'e2-medium', 'changed': True})
        self.assertEqual(self.provider.calls, ['stop', 'set-machine-type', 'start'])
        self.assertEqual(self.gates, ['acquire', 'release'])

    def test_start_failure_restores_original_and_releases(self):
        provider = Provider('start')
        with self.assertRaises(resize.ResizeError) as caught:
            resize.resize(provider, {}, 'e2-medium', lambda _: True, self.gate, lambda _: None)
        self.assertEqual((caught.exception.code, caught.exception.rollback, caught.exception.gate), ('vm_start_failed', 'restored', 'released'))
        self.assertEqual(provider.calls, ['stop', 'set-machine-type', 'start', 'stop', 'set-machine-type', 'start'])
        self.assertEqual(self.gates, ['acquire', 'release'])

    def test_machine_type_failure_restores_original(self):
        provider = Provider('set-machine-type')
        with self.assertRaises(resize.ResizeError) as caught:
            resize.resize(provider, {}, 'e2-medium', lambda _: True, self.gate, lambda _: None)
        self.assertEqual((caught.exception.code, caught.exception.rollback), ('resize_failed', 'restored'))
        self.assertEqual(self.gates, ['acquire', 'release'])

    def test_gateway_failure_rolls_back_and_preserves_gate_if_unconfirmed(self):
        calls = iter([True] + [False] * 48)
        with self.assertRaises(resize.ResizeError) as caught:
            resize.resize(self.provider, {}, 'e2-medium', lambda _: next(calls), self.gate, lambda _: None)
        self.assertEqual((caught.exception.code, caught.exception.rollback, caught.exception.gate), ('health_not_ready', 'unconfirmed', 'retained'))
        self.assertEqual(self.gates, ['acquire'])

    def test_unhealthy_before_lock_does_not_change_vm(self):
        with self.assertRaises(resize.ResizeError) as caught:
            resize.resize(self.provider, {}, 'e2-medium', lambda _: False, self.gate, lambda _: None)
        self.assertEqual(caught.exception.code, 'health_not_ready')
        self.assertFalse(self.provider.calls)
        self.assertFalse(self.gates)

    def test_release_failure_rolls_back_only_when_gate_confirmed_closed(self):
        def fail_once(provider, action):
            self.gates.append(action)
            if self.gates == ['acquire', 'release']:
                raise RuntimeError('fixture release failed')
            if action == 'status':
                return True
        with self.assertRaises(resize.ResizeError) as caught:
            resize.resize(self.provider, {}, 'e2-medium', lambda _: True, fail_once, lambda _: None)
        self.assertEqual((caught.exception.code, caught.exception.rollback), ('resize_failed', 'restored'))
        self.assertEqual(self.gates, ['acquire', 'release', 'status', 'release'])

    def test_foreign_state_rejected_without_creating_file(self):
        with tempfile.TemporaryDirectory() as tmp:
            path = Path(tmp) / 'state.json'
            with self.assertRaises(resize.ResizeError):
                resize.load_state(path, 'a', 'p')
            self.assertFalse(path.exists())
            path.write_text(json.dumps({'version': 1, 'id': '0123456789ab', 'account': 'a', 'project': 'p'}), encoding='utf-8')
            with self.assertRaises(resize.ResizeError):
                resize.load_state(path, 'other', 'p')

    def test_gateway_url_and_session_are_restricted(self):
        for url, token in [('http://example.test', 'token'), ('https://user@example.test', 'token'), ('https://example.test/x', 'token'), ('https://example.test', 'bad; cookie')]:
            self.assertFalse(resize.gateway_probe(url, token))


if __name__ == '__main__':
    unittest.main()
