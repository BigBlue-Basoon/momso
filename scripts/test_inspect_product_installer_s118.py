import importlib.util
import os
from pathlib import Path
import tempfile
import unittest
from unittest.mock import patch

spec = importlib.util.spec_from_file_location("inspect_product", Path(__file__).with_name("inspect-product-installer-s118.py"))
module = importlib.util.module_from_spec(spec)
spec.loader.exec_module(module)


class ProductInspectionSafetyTests(unittest.TestCase):
    def test_uses_explicit_cargo_target(self):
        with tempfile.TemporaryDirectory() as directory, patch.dict(os.environ, {'CARGO_TARGET_DIR': directory}):
            self.assertEqual(module.build_target(), Path(directory).resolve())

    def test_refuses_existing_install_process_startup_or_shortcut(self):
        with patch.object(module, "key_exists", return_value=False), patch.object(module, "run_value_exists", return_value=False), patch.object(module, "app_running", return_value=False), patch.object(module, "shortcut_exists", return_value=False):
            module.ensure_safe_state()
            for name in ("key_exists", "run_value_exists", "app_running", "shortcut_exists"):
                with self.subTest(name=name), patch.object(module, name, return_value=True):
                    with self.assertRaises(ValueError):
                        module.ensure_safe_state()


if __name__ == "__main__":
    unittest.main()
