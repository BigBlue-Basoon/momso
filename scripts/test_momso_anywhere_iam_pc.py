import json
import unittest
from pathlib import Path
from momso_anywhere_iam_pc import remove_installation_bindings, inspect_installation_bindings


class Provider:
    install_id = '0123456789ab'; project = 'fixture-project'; marker = 'momso-install-' + install_id
    def __init__(self):
        self.calls = []
        self.email = 'mv-' + self.install_id + '@' + self.project + '.iam.gserviceaccount.com'
        self.policy = {'etag': 'fixture', 'version': 3, 'bindings': [{'role': 'fixture', 'members': ['serviceAccount:' + self.email, 'deleted:serviceAccount:' + self.email + '?uid=123', 'user:other@example.test']}]}
        self.policies = {}
    def run(self, *args, parse=False, **_kwargs):
        self.calls.append(args)
        if args[:3] == ('iam', 'service-accounts', 'list'): return [{'email': self.email}]
        if args[:3] == ('iam', 'service-accounts', 'describe'): return {'description': self.marker}
        if args[:3] == ('compute', 'instances', 'list'): return [{'name': 'momso-' + self.install_id, 'labels': {'momso-install-id': self.install_id}}]
        if args[:3] == ('run', 'services', 'list'): return [{'metadata': {'name': 'momso-any-' + self.install_id, 'labels': {'momso-install-id': self.install_id}}}]
        if args[:3] == ('storage', 'buckets', 'list'): return [{'name': 'momso-backup-' + self.install_id}]
        if args[:2] == ('storage', 'ls'): return b'[]'
        if args[:3] == ('storage', 'buckets', 'describe'): return {'labels': {'momso-install-id': self.install_id}}
        if args[:2] == ('secrets', 'list'): return [{'name': 'projects/x/secrets/momso-vault-' + self.install_id, 'labels': {'momso-install-id': self.install_id}}]
        if args[:3] == ('kms', 'keyrings', 'list'): return [{'name': 'projects/x/locations/x/keyRings/momso-backup-' + self.install_id}]
        if args[:3] == ('kms', 'keys', 'list'): return [{'name': 'projects/x/cryptoKeys/backup'}]
        if args[:3] == ('kms', 'keys', 'describe'): return {'labels': {'momso-install-id': self.install_id}}
        key = tuple(item for item in args if item not in ('get-iam-policy', 'set-iam-policy') and not item.endswith('.json'))
        if 'get-iam-policy' in args: return json.loads(json.dumps(self.policies.get(key, self.policy)))
        if 'set-iam-policy' in args:
            path = next(Path(item) for item in args if item.endswith('.json'))
            self.policies[key] = json.loads(path.read_text(encoding='utf-8'))
            return b''
        raise AssertionError(args)


class Tests(unittest.TestCase):
    def test_inventory_rechecks_project_bucket_keyring_key_and_secret(self):
        p = Provider()
        found = inspect_installation_bindings(p)
        self.assertEqual(len(found), 7)
        calls = [call for call in p.calls if 'get-iam-policy' in call]
        self.assertEqual(len(calls), 7)
        self.assertTrue(any(call[:3] == ('storage', 'buckets', 'get-iam-policy') for call in calls))
        self.assertTrue(any(call[:3] == ('kms', 'keys', 'get-iam-policy') for call in calls))
        self.assertTrue(any(call[:2] == ('secrets', 'get-iam-policy') for call in calls))
        self.assertFalse(any('set-iam-policy' in call for call in p.calls))

    def test_soft_deleted_bucket_policy_is_not_claimed_absent(self):
        p = Provider(); original = p.run
        def run(*args, **kwargs):
            if args[:2] == ('storage', 'ls'):
                return json.dumps([{'metadata': {'name': 'momso-backup-' + p.install_id}}]).encode()
            return original(*args, **kwargs)
        p.run = run
        with self.assertRaisesRegex(ValueError, 'soft_deleted_bucket_policy_unavailable') as caught:
            inspect_installation_bindings(p)
        self.assertTrue(caught.exception.found)
        self.assertFalse(any('set-iam-policy' in call for call in p.calls))
    def test_policy_removal_is_verified_for_each_owned_resource(self):
        p = Provider(); remove_installation_bindings(p)
        self.assertEqual(len(p.policies), 8)
        for policy in p.policies.values(): self.assertEqual(policy['bindings'][0]['members'], ['user:other@example.test'])
        reads = [call for call in p.calls if 'get-iam-policy' in call]
        self.assertEqual(len(reads), 16)
        self.assertFalse(any('delete' in call for call in p.calls))

    def test_unowned_account_fails_before_policy_write(self):
        p = Provider(); original = p.run
        def run(*args, **kwargs):
            if args[:3] == ('iam', 'service-accounts', 'describe'): return {'description': 'foreign'}
            return original(*args, **kwargs)
        p.run = run
        with self.assertRaisesRegex(ValueError, 'resource_ownership'): remove_installation_bindings(p)
        self.assertFalse(any('set-iam-policy' in call for call in p.calls))
