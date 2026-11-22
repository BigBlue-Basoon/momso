import json
from pathlib import Path
import tempfile
import unittest

from progress_status import ProgressFile


class ProgressStatusTests(unittest.TestCase):
    def test_atomic_status_has_only_allowed_values(self):
        with tempfile.TemporaryDirectory() as directory:
            path = Path(directory) / "status.json"
            progress = ProgressFile(path)
            progress({"state": "installing", "reason": None, "private": "discarded"})
            self.assertEqual(json.loads(path.read_text()), {"state": "installing", "reason": None})
            with self.assertRaisesRegex(ValueError, "invalid_progress"):
                progress({"state": "failed", "reason": "/private/path"})
            self.assertEqual(json.loads(path.read_text())["state"], "installing")
