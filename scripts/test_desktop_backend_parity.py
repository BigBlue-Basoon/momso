import importlib.util
import json
from pathlib import Path
import tempfile
import unittest
from unittest.mock import patch

SCRIPT = Path(__file__).with_name("desktop-backend-parity.py")
spec = importlib.util.spec_from_file_location("backend_parity", SCRIPT)
parity = importlib.util.module_from_spec(spec)
spec.loader.exec_module(parity)


class ParityTest(unittest.TestCase):
    def test_sync_preserves_screen_and_generated_files(self):
        with tempfile.TemporaryDirectory() as temp:
            root = Path(temp) / "source"
            screen = Path(temp) / "screen"
            for directory in (root, screen):
                (directory / "apps/alpha/desktop/src-tauri/src").mkdir(parents=True)
                (directory / "apps/alpha/desktop/src").mkdir(parents=True)
                (directory / ".gitattributes").write_text("", encoding="utf-8")
                (directory / "apps/alpha/desktop/package.json").write_text(
                    json.dumps({"scripts": {"build:bundle": "node scripts/prepare-node.mjs && npm run build"}}), encoding="utf-8"
                )
            (root / "apps/alpha/desktop/src-tauri/src/example.rs").write_text("new", encoding="utf-8")
            (screen / "apps/alpha/desktop/src-tauri/src/example.rs").write_text("old", encoding="utf-8")
            protected = screen / "apps/alpha/desktop/src/screen.ts"
            protected.write_text("keep", encoding="utf-8")
            generated = screen / "apps/alpha/desktop/src-tauri/generated/node/node.exe"
            generated.parent.mkdir(parents=True)
            generated.write_bytes(b"keep")
            placeholder = root / "apps/alpha/desktop/src-tauri/generated/node/.gitkeep"
            placeholder.parent.mkdir(parents=True)
            placeholder.write_bytes(b"")
            obsolete = screen / "apps/alpha/desktop/scripts/prepare-mingit.mjs"
            obsolete.parent.mkdir(parents=True)
            obsolete.write_text("old", encoding="utf-8")
            with patch.object(parity, "ROOT", root), patch("sys.argv", ["parity", str(screen)]):
                self.assertEqual(parity.main(), 1)
            with patch.object(parity, "ROOT", root), patch("sys.argv", ["parity", str(screen), "--sync"]):
                self.assertEqual(parity.main(), 0)
            self.assertEqual(protected.read_text(encoding="utf-8"), "keep")
            self.assertEqual(generated.read_bytes(), b"keep")
            self.assertTrue((generated.parent / ".gitkeep").is_file())
            self.assertFalse(obsolete.exists())
            self.assertEqual((screen / "apps/alpha/desktop/src-tauri/src/example.rs").read_text(encoding="utf-8"), "new")
            (root / ".gitignore").write_text("", encoding="utf-8")
            (screen / ".gitignore").write_text("", encoding="utf-8")
            self.assertFalse(parity.check_ignore_rules(screen))

    def test_private_templates_and_docs_are_never_synced(self):
        with tempfile.TemporaryDirectory() as temp:
            root, screen = Path(temp) / 'source', Path(temp) / 'screen'
            for directory in (root, screen):
                directory.mkdir()
                package = directory / 'apps/alpha/desktop/package.json'
                package.parent.mkdir(parents=True)
                package.write_text(json.dumps({'scripts': {'build:bundle': 'node scripts/prepare-node.mjs && npm run build'}}), encoding='utf-8')
            for name in ('.github/ISSUE_TEMPLATE/private.md', '.github/pull_request_template.md', '.github/workflows/momso-server-linux.yml', 'docs/development/private.md'):
                path = root / name
                path.parent.mkdir(parents=True, exist_ok=True)
                path.write_text('private', encoding='utf-8')
            with patch.object(parity, 'ROOT', root), patch('sys.argv', ['parity', str(screen), '--sync']):
                self.assertEqual(parity.main(), 0)
            self.assertFalse((screen / '.github').exists())
            self.assertFalse((screen / 'docs').exists())
            private = screen / '.github/pull_request_template.md'
            private.parent.mkdir(parents=True)
            private.write_text('preserve', encoding='utf-8')
            with patch.object(parity, 'ROOT', root), patch('sys.argv', ['parity', str(screen), '--sync']):
                self.assertEqual(parity.main(), 1)
            self.assertEqual(private.read_text(encoding='utf-8'), 'preserve')

    def test_unknown_screen_file_is_never_deleted(self):
        with tempfile.TemporaryDirectory() as temp:
            root = Path(temp) / "source"
            screen = Path(temp) / "screen"
            root.mkdir()
            extra = screen / "apps/alpha/desktop/src-tauri/src/unknown.rs"
            extra.parent.mkdir(parents=True)
            extra.write_text("keep", encoding="utf-8")
            with patch.object(parity, "ROOT", root), patch("sys.argv", ["parity", str(screen), "--sync"]):
                self.assertEqual(parity.main(), 1)
            self.assertTrue(extra.exists())


if __name__ == "__main__":
    unittest.main()
