import json
import unittest
from pathlib import Path
from momso_anywhere_iam_pc import remove_installation_bindings, inspect_installation_bindings, soft_deleted_buckets
from momso_server_install import InstallError


def real_soft_deleted_ls(args, names):
    """Cloud SDK 559 shape: --json/--full answer HTTP 404, the plain listing
    prints one gs://NAME/ line per soft-deleted bucket."""
    if '--json' in args or '--full' in args or any(item.startswith('--format') for item in args):
        raise InstallError('ls failed', http_status=404)
    if not names:
        # Real gcloud: exit 1, empty stdout, the exact no-match line on stderr.
        raise InstallError('ls failed', no_match=True)
    return ''.join('gs://' + name + '/\n' for name in names).encode()


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
        if args[:2] == ('storage', 'ls'): return real_soft_deleted_ls(args, [])
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
                return real_soft_deleted_ls(args, ['other-deleted-bucket', 'momso-backup-' + p.install_id])
            return original(*args, **kwargs)
        p.run = run
        with self.assertRaisesRegex(ValueError, 'soft_deleted_bucket_policy_unavailable') as caught:
            inspect_installation_bindings(p)
        self.assertTrue(caught.exception.found)
        self.assertFalse(any('set-iam-policy' in call for call in p.calls))
    def test_soft_deleted_listing_uses_plain_lines_and_matches_exact_name(self):
        p = Provider(); original = p.run
        listing = ['momso-backup-' + p.install_id + 'x', 'momso-backup-' + p.install_id, 'momso-backup-' + p.install_id]
        def run(*args, **kwargs):
            if args[:2] == ('storage', 'ls'):
                return real_soft_deleted_ls(args, listing)
            return original(*args, **kwargs)
        p.run = run
        self.assertEqual(soft_deleted_buckets(p), [{'name': 'momso-backup-' + p.install_id, 'hardDeleteTime': None}])
        listing[:] = ['momso-backup-' + p.install_id + 'x', 'unrelated']
        self.assertEqual(soft_deleted_buckets(p), [])
        listing[:] = []
        self.assertEqual(soft_deleted_buckets(p), [])
        def failing(*args, **kwargs):
            if args[:2] == ('storage', 'ls'): raise InstallError('ls failed', http_status=403)
            return original(*args, **kwargs)
        p.run = failing
        with self.assertRaises(InstallError):
            soft_deleted_buckets(p)
        def odd(*args, **kwargs):
            if args[:2] == ('storage', 'ls'): return b'something unexpected\n'
            return original(*args, **kwargs)
        p.run = odd
        with self.assertRaisesRegex(ValueError, 'invalid_bucket_inventory'):
            soft_deleted_buckets(p)

    def test_install_that_never_enabled_cloud_run_still_inventories(self):
        p = Provider(); original = p.run
        def run(*args, **kwargs):
            if args[:3] == ('run', 'services', 'list'):
                raise InstallError('cloud failure', service_disabled=True)
            return original(*args, **kwargs)
        p.run = run
        found = inspect_installation_bindings(p)
        self.assertTrue(found)
        self.assertFalse(any(call[:3] == ('run', 'services', 'get-iam-policy') for call in p.calls))

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
