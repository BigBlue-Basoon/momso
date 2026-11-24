import importlib.util
from pathlib import Path
import unittest

spec = importlib.util.spec_from_file_location("startup", Path(__file__).with_name("verify-product-startup-s118.py"))
startup = importlib.util.module_from_spec(spec)
spec.loader.exec_module(startup)


class IsolatedUserTests(unittest.TestCase):
    def test_requires_different_well_formed_user_sid(self):
        row = '"fixture\\other","S-1-5-21-2"'
        startup.require_distinct_sid(row, "S-1-5-21-1")
        with self.assertRaisesRegex(ValueError, "separate"):
            startup.require_distinct_sid(row, "S-1-5-21-2")
        with self.assertRaisesRegex(ValueError, "valid"):
            startup.require_distinct_sid(row, "other")


if __name__ == "__main__":
    unittest.main()
