import io
import json
import tarfile
import tempfile
import unittest
from pathlib import Path

import momso_anywhere_public_assets as assets
import prepare_momso_anywhere_image as image


def fixture():
    result = {name: b"\x89PNG\r\n\x1a\nfixture" for name in assets.ASSETS if name.endswith(".png")}
    result["manifest.webmanifest"] = json.dumps({"name": "momso", "start_url": "/", "scope": "/", "icons": [{"src": "/app-icons/icon-192.png"}]}).encode()
    return result


class Tests(unittest.TestCase):
    def test_directory_and_archive_allow_only_known_pwa_files(self):
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            web = root / "web"
            assets.write_assets(fixture(), web)
            (web / "private.json").write_text("fixture", encoding="ascii")
            self.assertEqual(assets.from_web_directory(web), fixture())
            archive = root / "web.tar.gz"
            with tarfile.open(archive, "w:gz") as tar:
                tar.add(web, arcname=".")
            self.assertEqual(assets.from_web_archive(archive), fixture())
            out = root / "context"
            image.prepare(web, out)
            self.assertTrue((out / "public/manifest.webmanifest").is_file())
            self.assertFalse((out / "public/private.json").exists())
            self.assertTrue((out / "scripts/momso_anywhere_image/Dockerfile").is_file())

    def test_manifest_cannot_make_a_private_path_public(self):
        files = fixture()
        files["manifest.webmanifest"] = json.dumps({"scope": "/", "start_url": "/", "icons": [{"src": "/private.json"}]}).encode()
        with self.assertRaises(ValueError): assets.validate(files)

    def test_archive_refuses_duplicate_asset(self):
        with tempfile.TemporaryDirectory() as directory:
            archive = Path(directory) / "fixture.tar.gz"
            with tarfile.open(archive, "w:gz") as tar:
                for name, data in fixture().items():
                    info = tarfile.TarInfo(name); info.size = len(data); tar.addfile(info, io.BytesIO(data))
                info = tarfile.TarInfo("manifest.webmanifest"); info.size = 2; tar.addfile(info, io.BytesIO(b"{}"))
            with self.assertRaises(ValueError): assets.from_web_archive(archive)


if __name__ == "__main__": unittest.main()
