#!/usr/bin/env python3
"""Check local Markdown link destinations in a public snapshot, without fetching URLs."""
import os
import sys
from html.parser import HTMLParser
from pathlib import Path
from urllib.parse import unquote, urlsplit

from markdown_it import MarkdownIt

ROOT = Path(__file__).resolve().parent.parent
# Run against a clean public snapshot. Only repository metadata and installed
# dependencies are outside its authored Markdown files.
SKIP = {'.git', 'node_modules', '__pycache__'}


class LocalHtmlLinks(HTMLParser):
    def __init__(self):
        super().__init__(convert_charrefs=True)
        self.links = []

    def handle_starttag(self, tag, attrs):
        field = {'a': 'href', 'img': 'src'}.get(tag.lower())
        if field:
            self.links.extend(value for key, value in attrs if key.lower() == field and value)

    handle_startendtag = handle_starttag


def markdown_files(root):
    for directory, dirs, names in os.walk(root, followlinks=False):
        dirs[:] = sorted(d for d in dirs if d not in SKIP and not (Path(directory) / d).is_symlink())
        for name in sorted(names):
            path = Path(directory) / name
            if path.suffix.lower() == '.md' and not path.is_symlink() and path.is_file():
                yield path


def exact_existing(root, source, destination):
    """Match repository paths case-sensitively even when running on Windows."""
    parts = urlsplit(destination)
    if parts.scheme or parts.netloc or not parts.path or parts.path.startswith('/'):
        return True
    raw = unquote(parts.path)
    if '\\' in raw or '\x00' in raw:
        return False
    current = source.parent.relative_to(root)
    for part in raw.split('/'):
        if part in ('', '.'):
            continue
        if part == '..':
            if not current.parts:
                return False
            current = current.parent
            continue
        parent = root / current
        try:
            if part not in os.listdir(parent):
                return False
        except (OSError, ValueError):
            return False
        current /= part
    target = root / current
    try:
        return target.resolve().is_relative_to(root.resolve()) and target.exists()
    except (OSError, ValueError, RuntimeError):
        return False


def destinations(tokens):
    for block in tokens:
        line = block.map[0] + 1 if block.map else 1
        children = block.children if block.children is not None else [block]
        for child in children:
            if child.type == 'link_open':
                yield line, child.attrGet('href')
            elif child.type == 'image':
                yield line, child.attrGet('src')
            elif child.type in ('html_inline', 'html_block'):
                parser = LocalHtmlLinks()
                parser.feed(child.content)
                for link in parser.links:
                    yield line, link


def findings(root):
    root = root.resolve(strict=True)
    parser = MarkdownIt('commonmark', {'html': True})
    result = []
    for source in markdown_files(root):
        text = source.read_text(encoding='utf-8-sig')
        lines = text.splitlines()
        for line, destination in destinations(parser.parse(text)):
            if destination and not exact_existing(root, source, destination):
                # Inline tokens carry the paragraph start; locate the destination
                # within it (or its reference definition) when possible.
                variants = (destination, unquote(destination))
                for index in range(line - 1, len(lines)):
                    if any(value in lines[index] for value in variants):
                        line = index + 1
                        break
                result.append((source.relative_to(root).as_posix(), line, destination))
    return result


def main():
    root = Path(sys.argv[1]) if len(sys.argv) > 1 else ROOT
    result = findings(root)
    for source, line, destination in result:
        print(f'{source}:{line}: missing relative target: {destination}')
    print(f'broken relative links: {len(result)}')
    return 1 if result else 0


if __name__ == '__main__':
    raise SystemExit(main())
