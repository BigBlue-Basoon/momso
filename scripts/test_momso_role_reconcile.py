import unittest
from momso_iam_retry import reconcile_owned_role


class RoleTests(unittest.TestCase):
    def provider(self, *, deleted=False, marker='fixture', permissions=('old',)):
        class Provider:
            calls = []
            def run(self, *args, **kwargs):
                self.calls.append(args)
                if args[:3] == ('iam', 'roles', 'list'):
                    return [{'name': 'projects/test/roles/momsoRole'}]
                if args[:3] == ('iam', 'roles', 'describe'):
                    return {'description': marker, 'deleted': deleted, 'includedPermissions': permissions}
                return b''
        p = Provider(); p.marker = 'fixture'
        return p

    def test_deleted_owned_role_is_undeleted_and_updated(self):
        p = self.provider(deleted=True)
        reconcile_owned_role(p, 'momsoRole', {'new'})
        self.assertIn(('iam', 'roles', 'undelete', 'momsoRole'), p.calls)
        self.assertIn(('iam', 'roles', 'update', 'momsoRole', '--permissions=new', '--stage=GA'), p.calls)

    def test_foreign_role_is_never_changed(self):
        p = self.provider(deleted=True, marker='foreign')
        with self.assertRaisesRegex(ValueError, 'role_ownership_mismatch'):
            reconcile_owned_role(p, 'momsoRole', {'new'})
        self.assertFalse(any('undelete' in command or 'update' in command for command in p.calls))
