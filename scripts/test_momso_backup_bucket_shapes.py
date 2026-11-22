import unittest

from momso_anywhere_backup_provision import GoogleBackupProvisioner, bucket_is_owned_and_private
from momso_server_install import InstallError


class BucketDescribeTests(unittest.TestCase):
    install_id = 'abcdef123456'

    def test_owned_private_bucket_accepts_supported_sdk_and_api_shapes(self):
        labels = {'momso-install-id': self.install_id}
        for iam in (
            {'public_access_prevention': 'enforced', 'uniform_bucket_level_access': True},
            {'public_access_prevention': 'enforced', 'uniform_bucket_level_access': {'enabled': True}},
            {'publicAccessPrevention': 'enforced', 'uniformBucketLevelAccess': True},
            {'publicAccessPrevention': 'enforced', 'uniformBucketLevelAccess': {'enabled': True}},
        ):
            for wrapper in (None, 'iam_config', 'iamConfiguration'):
                with self.subTest(iam=iam, wrapper=wrapper):
                    body = {'labels': labels, **(iam if wrapper is None else {wrapper: iam})}
                    self.assertTrue(bucket_is_owned_and_private(body, self.install_id))

    def test_missing_false_conflicting_or_foreign_settings_are_rejected(self):
        valid = {'labels': {'momso-install-id': self.install_id},
                 'public_access_prevention': 'enforced', 'uniform_bucket_level_access': True}
        for change in (
            {'labels': {'momso-install-id': 'other'}},
            {'public_access_prevention': 'inherited'},
            {'uniform_bucket_level_access': False},
            {'uniform_bucket_level_access': {'enabled': False}},
            {'uniform_bucket_level_access': 'true'},
            {'iamConfiguration': {'publicAccessPrevention': 'inherited', 'uniformBucketLevelAccess': {'enabled': True}}},
            {'iam_config': None},
        ):
            with self.subTest(change=change):
                self.assertFalse(bucket_is_owned_and_private({**valid, **change}, self.install_id))
        self.assertFalse(bucket_is_owned_and_private({'labels': valid['labels']}, self.install_id))

    def test_existing_sdk_bucket_is_reused_without_modifying_it(self):
        class Provider:
            install_id = 'abcdef123456'
            project = 'fixture-project'
            marker = 'momso-install-' + install_id
            def __init__(self): self.calls = []
            def run(self, *args, **kwargs):
                self.calls.append(args)
                if args[:3] == ('storage', 'buckets', 'list'):
                    return [{'name': 'momso-backup-' + self.install_id}]
                if args[:3] == ('storage', 'buckets', 'describe'):
                    return {'labels': {'momso-install-id': self.install_id},
                            'public_access_prevention': 'enforced', 'uniform_bucket_level_access': True}
                return [] if kwargs.get('parse') else b''
        provider = Provider()
        GoogleBackupProvisioner(provider).provision()
        self.assertFalse(any(call[:3] in (('storage', 'buckets', 'create'), ('storage', 'buckets', 'update')) for call in provider.calls))

    def test_foreign_bucket_is_not_modified(self):
        class Provider:
            install_id = 'abcdef123456'
            project = 'fixture-project'
            marker = 'momso-install-' + install_id
            def run(self, *args, **kwargs):
                if args[:3] == ('storage', 'buckets', 'list'): return [{'name': 'momso-backup-' + self.install_id}]
                if args[:3] == ('storage', 'buckets', 'describe'):
                    return {'labels': {'momso-install-id': 'other'}, 'public_access_prevention': 'enforced', 'uniform_bucket_level_access': True}
                raise AssertionError('foreign bucket must not be changed')
        with self.assertRaises(InstallError):
            GoogleBackupProvisioner(Provider()).provision()


if __name__ == '__main__':
    unittest.main()
