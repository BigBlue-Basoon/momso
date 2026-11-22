import unittest

import check_momso_gcloud_options as check


class GcloudOptionCatalogTests(unittest.TestCase):
    def test_installer_bucket_flags_follow_real_sdk_command_boundaries(self):
        commands = check.catalog()
        create = commands[('storage', 'buckets', 'create')]
        update = commands[('storage', 'buckets', 'update')]
        self.assertNotIn('--labels', create)
        self.assertNotIn('--update-labels', create)
        self.assertIn('--update-labels', update)

    def test_detects_negative_flag_spelling_and_missing_flags(self):
        text = 'OPTIONS --[no-]public-access-prevention --update-labels=LABELS'
        self.assertTrue(check.supports(text, '--no-public-access-prevention'))
        self.assertTrue(check.supports(text, '--update-labels'))
        self.assertFalse(check.supports(text, '--labels'))

    def test_variable_destroy_and_gateway_commands_are_covered(self):
        commands = check.catalog()
        self.assertIn('--to-revisions', commands[('run', 'services', 'update-traffic')])
        self.assertIn('--set-secrets', commands[('run', 'deploy')])
        self.assertIn('--zone', commands[('compute', 'instances', 'delete')])
        self.assertIn('--region', commands[('compute', 'networks', 'subnets', 'delete')])


if __name__ == '__main__':
    unittest.main()
