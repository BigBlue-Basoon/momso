import importlib.util
from pathlib import Path
import unittest

spec = importlib.util.spec_from_file_location("channels", Path(__file__).with_name("momso-release-channels.py"))
channels = importlib.util.module_from_spec(spec)
spec.loader.exec_module(channels)


class ReleaseChannelTests(unittest.TestCase):
    def test_anywhere_cannot_become_latest(self):
        for prerelease in (False, True):
            command = channels.anywhere_command("anywhere-v1.2.3", ["release.json"], prerelease)
            self.assertIn("--latest=false", command)
            self.assertEqual("--prerelease" in command, prerelease)
        with self.assertRaises(ValueError):
            channels.anywhere_command("v1.2.3", ["release.json"])
        with self.assertRaises(ValueError):
            channels.anywhere_command("anywhere-v1", ["--latest=true"])

    def test_desktop_latest_requires_expected_tag_and_assets(self):
        release = {"tag_name": "v0.1.1", "draft": False, "prerelease": False,
                   "assets": [{"name": "momso_0.1.1_x64-setup.exe", "state": "uploaded"},
                              {"name": "latest.json", "state": "uploaded"}]}
        self.assertEqual(channels.verify_desktop(release, "v0.1.1"), "momso_0.1.1_x64-setup.exe")
        for change in ({"tag_name": "anywhere-v1"}, {"prerelease": True}, {"assets": []}):
            with self.assertRaises(ValueError):
                channels.verify_desktop({**release, **change}, "v0.1.1")


if __name__ == "__main__":
    unittest.main()
