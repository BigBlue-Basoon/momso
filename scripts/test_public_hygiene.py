import importlib.util
import tempfile
import unittest
from pathlib import Path

spec = importlib.util.spec_from_file_location("public_hygiene", Path(__file__).with_name("check-public-hygiene.py"))
hygiene = importlib.util.module_from_spec(spec)
spec.loader.exec_module(hygiene)


class PublicHygieneTest(unittest.TestCase):
    def test_patterns_and_external_terms(self):
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            (root / "apps").mkdir()
            source = root / "apps" / "fixture.txt"
            source.write_text("C:" + chr(92) + "Users" + chr(92) + "someone\n" + "person" + "@example.net\n" + "private-marker\n" + "sk-proj-" + "a" * 20, encoding="utf-8")
            self.assertEqual([x[2] for x in hygiene.findings(root, ["private-marker"])], ["user_path", "email", "private_term", "key"])

    def test_fake_addresses_and_generated_folders(self):
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            source = root / "apps" / "target"
            source.mkdir(parents=True)
            (source / "ignored.txt").write_text("person" + "@example.net", encoding="utf-8")
            (root / "apps" / "safe.txt").write_text("test" + "@example.invalid", encoding="utf-8")
            self.assertEqual(hygiene.findings(root, []), [])

    def test_forward_slash_user_path(self):
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            (root / "apps").mkdir()
            (root / "apps" / "fixture.txt").write_text("C:/" + "Users/someone/data", encoding="utf-8")
            self.assertEqual(hygiene.findings(root, [])[0][2], "user_path")


if __name__ == "__main__":
    unittest.main()
