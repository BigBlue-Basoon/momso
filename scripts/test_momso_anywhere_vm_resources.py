import unittest
from unittest.mock import patch
import momso_anywhere_vm_resources as resources
from momso_anywhere_vm_resources import destroy_identity, names


class Provider:
    install_id = '0123456789ab'
    project = 'fixture-project'
    marker = 'fixture-owned'

    def __init__(self):
        n = names(self)
        self.account = {'email': n['account'] + '@' + self.project + '.iam.gserviceaccount.com', 'description': self.marker}
        self.roles = [{'name': 'projects/fixture/roles/' + x, 'description': self.marker} for x in n['roles']]
        self.secret = {'name': 'projects/fixture/secrets/' + n['vault_secret'], 'labels': {'momso-install-id': self.install_id}}
        self.deleted = []

    def run(self, *args, parse=False):
        if args[:3] == ('iam', 'service-accounts', 'list'): return [self.account] if self.account else []
        if args[:3] == ('iam', 'roles', 'list'): return self.roles
        if args[:2] == ('secrets', 'list'): return [self.secret] if self.secret else []
        if args[:3] == ('iam', 'service-accounts', 'describe'): return self.account
        if args[:3] == ('iam', 'roles', 'describe'): return next(x for x in self.roles if x['name'].endswith('/' + args[3]))
        self.deleted.append(args)
        return b''


class ResourceTests(unittest.TestCase):
    def test_pause_preserves_vault_and_deletes_vm_identity(self):
        p = Provider(); destroy_identity(p, discard_data=False)
        self.assertEqual(len(p.deleted), 1)
        self.assertFalse(any(call[:3] == ('iam', 'roles', 'delete') for call in p.deleted))
        self.assertFalse(any(call[0] == 'secrets' for call in p.deleted))

    def test_discard_deletes_owned_vault_and_vm_identity(self):
        p = Provider(); destroy_identity(p, discard_data=True)
        self.assertEqual(len(p.deleted), 5)
        self.assertIn(('secrets', 'delete', names(p)['vault_secret']), p.deleted)

    def test_foreign_role_aborts_before_any_delete(self):
        p = Provider(); p.roles[0]['description'] = 'another-owner'
        with self.assertRaises(ValueError): destroy_identity(p, discard_data=True)
        self.assertEqual(p.deleted, [])

    def test_foreign_vault_aborts_before_any_delete(self):
        p = Provider(); p.secret['labels'] = {'momso-install-id': 'other'}
        with self.assertRaises(ValueError): destroy_identity(p, discard_data=True)
        self.assertEqual(p.deleted, [])

    def test_added_declared_role_is_cleaned_without_new_deletion_logic(self):
        p = Provider(); declared = names(p)
        declared['roles'] = (*declared['roles'], 'fixtureExtra' + p.install_id)
        p.roles.append({'name': 'projects/fixture/roles/' + declared['roles'][-1], 'description': p.marker})
        with patch.object(resources, 'names', return_value=declared):
            destroy_identity(p, discard_data=True)
        self.assertIn(('iam', 'roles', 'delete', declared['roles'][-1]), p.deleted)
