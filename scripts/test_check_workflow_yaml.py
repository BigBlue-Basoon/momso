import importlib.util
from pathlib import Path
import tempfile
import unittest


spec = importlib.util.spec_from_file_location("check_workflow_yaml", Path(__file__).with_name("check-workflow-yaml.py"))
checker = importlib.util.module_from_spec(spec)
spec.loader.exec_module(checker)


class WorkflowYamlTests(unittest.TestCase):
    def test_valid_workflow_and_both_extensions(self):
        with tempfile.TemporaryDirectory() as directory:
            path = Path(directory)
            for name in ("one.yml", "two.yaml"):
                (path / name).write_text("on:\n  workflow_dispatch:\njobs:\n  build: {}\n", encoding="utf-8")
            self.assertEqual(checker.check(path), 2)

    def test_rejects_invalid_yaml_and_missing_required_keys(self):
        with tempfile.TemporaryDirectory() as directory:
            path = Path(directory)
            file = path / "bad.yml"
            for content in ("on: [\njobs: {}\n", "on: push\n", "jobs: {}\n", "true: push\njobs:\n  build: {}\n"):
                file.write_text(content, encoding="utf-8")
                with self.assertRaises(ValueError):
                    checker.check(path)


if __name__ == "__main__":
    unittest.main()
