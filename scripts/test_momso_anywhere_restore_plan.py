import unittest

from momso_anywhere_restore_plan import RestorePlanError, restore_plan


class RestorePlanTests(unittest.TestCase):
    def test_fresh_partial_unknown_and_preserved_are_distinct(self):
        self.assertEqual(restore_plan(vault='absent', bucket='absent', key='absent', slots=None), {'restoreAvailable': False, 'lastBackupAt': None, 'requiresRestore': False})
        for resources, error in [(('absent', 'ready', 'ready'), 'restore_assets_incomplete'), (('ready', 'unverified', 'ready'), 'restore_assets_unverified')]:
            with self.assertRaisesRegex(RestorePlanError, error):
                restore_plan(vault=resources[0], bucket=resources[1], key=resources[2], slots=None)
        self.assertEqual(restore_plan(vault='ready', bucket='ready', key='ready', slots=[]),
                         {'restoreAvailable': False, 'lastBackupAt': None, 'requiresRestore': True})
        slots = [{'name': 'momso-backup/slot-0.enc', 'updated': '2026-11-01T00:00:00Z'}, {'name': 'momso-backup/slot-3.enc', 'updated': '2026-11-02T00:00:00Z'}]
        self.assertEqual(restore_plan(vault='ready', bucket='ready', key='ready', slots=slots), {'restoreAvailable': True, 'lastBackupAt': '2026-11-02T00:00:00Z', 'requiresRestore': True})


if __name__ == '__main__': unittest.main()
