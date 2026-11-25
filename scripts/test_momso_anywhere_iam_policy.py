import copy
import unittest
from momso_anywhere_iam_policy import without_accounts, remove_and_verify

EMAIL = 'fixture@example.test'


class PolicyTests(unittest.TestCase):
    def test_removes_exact_live_and_deleted_members_preserving_others_and_conditions(self):
        policy = {'etag': 'fixture', 'version': 3, 'bindings': [
            {'role': 'role', 'condition': {'title': 'keep'}, 'members': [
                'serviceAccount:' + EMAIL, 'deleted:serviceAccount:' + EMAIL + '?uid=123',
                'serviceAccount:other@example.test']} ]}
        result = without_accounts(policy, [EMAIL])
        self.assertEqual(result['bindings'][0]['members'], ['serviceAccount:other@example.test'])
        self.assertEqual(result['bindings'][0]['condition'], {'title': 'keep'})
        self.assertEqual(len(policy['bindings'][0]['members']), 3)

    def test_policy_is_written_then_rechecked_before_caller_can_delete(self):
        class Transport:
            policy = {'etag': 'fixture', 'bindings': [{'role': 'role', 'members': ['serviceAccount:' + EMAIL]}]}
            events = []
            def read(self, resource): self.events.append(('read', resource)); return copy.deepcopy(self.policy)
            def write(self, resource, policy): self.events.append(('write', resource)); self.policy = policy
        t = Transport(); remove_and_verify(t, ['fixture'], [EMAIL])
        t.events.append(('delete-account', EMAIL))
        self.assertEqual([event[0] for event in t.events], ['read', 'write', 'read', 'delete-account'])

    def test_unremoved_deleted_principal_and_missing_etag_fail_closed(self):
        class Transport:
            def read(self, _): return {'etag': 'fixture', 'bindings': [{'role': 'role', 'members': ['deleted:serviceAccount:' + EMAIL + '?uid=123']}]}
            def write(self, *_): pass
        with self.assertRaisesRegex(ValueError, 'iam_bindings_unverified'):
            remove_and_verify(Transport(), ['fixture'], [EMAIL])
        with self.assertRaisesRegex(ValueError, 'policy_unverified'):
            without_accounts({'bindings': []}, [EMAIL])
