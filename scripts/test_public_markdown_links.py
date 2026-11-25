import importlib.util
import tempfile
import unittest
from pathlib import Path

spec = importlib.util.spec_from_file_location('public_links', Path(__file__).with_name('check-public-markdown-links.py'))
links = importlib.util.module_from_spec(spec)
spec.loader.exec_module(links)


class PublicMarkdownLinksTest(unittest.TestCase):
    def test_links_images_references_and_html(self):
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            docs = root / 'docs'; docs.mkdir()
            (root / 'exists (one).md').write_text('ok', encoding='utf8')
            (docs / 'README.md').write_text(
                '[ok](../exists%20%28one%29.md#part) [bad](missing.md)\n'
                '![image](missing.png) [ref][source]\n\n[source]: absent.md\n'
                '<a href="../gone.md">gone</a> <img src="missing.svg">\n', encoding='utf8')
            result = links.findings(root)
            self.assertEqual({item[2] for item in result},
                             {'missing.md', 'missing.png', 'absent.md', '../gone.md', 'missing.svg'})

    def test_ignores_external_fragments_and_code(self):
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            (root / 'README.md').write_text(
                '[web](https://example.com/no.md) [mail](mailto:test@example.com) '
                '[fragment](#section) [root](/route) [network](//example.com/x)\n'
                '`[inline](missing.md)`\n\n```md\n[code](missing.md)\n```\n', encoding='utf8')
            self.assertEqual(links.findings(root), [])

    def test_case_sensitive_and_outside_root(self):
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            (root / 'Correct.md').write_text('ok', encoding='utf8')
            (root / 'README.md').write_text('[case](correct.md) [escape](../elsewhere.md)', encoding='utf8')
            self.assertEqual({item[2] for item in links.findings(root)}, {'correct.md', '../elsewhere.md'})

    def test_nested_directories_and_query(self):
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            (root / 'docs').mkdir()
            (root / 'docs' / 'index.md').write_text('[directory](../docs/) [self](index.md?x=1#part)', encoding='utf8')
            self.assertEqual(links.findings(root), [])

    def test_reports_the_line_with_the_destination(self):
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            (root / 'README.md').write_text('first line\n[second](missing.md)\n', encoding='utf8')
            self.assertEqual(links.findings(root), [('README.md', 2, 'missing.md')])

    def test_generated_folders_are_not_examined(self):
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            (root / 'node_modules').mkdir()
            (root / 'node_modules' / 'generated.md').write_text('[bad](missing.md)', encoding='utf8')
            (root / 'nested').mkdir()
            (root / 'nested' / 'actual.md').write_text('[bad](missing.md)', encoding='utf8')
            (root / 'dist').mkdir()
            (root / 'dist' / 'included.md').write_text('[bad](missing.md)', encoding='utf8')
            self.assertEqual({item[0] for item in links.findings(root)}, {'nested/actual.md', 'dist/included.md'})


if __name__ == '__main__':
    unittest.main()
