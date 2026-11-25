"""The VM backup and restore files delivered by every installation path."""
import json
from pathlib import Path

RESTORE_FILES = tuple(json.loads(Path(__file__).with_suffix('.json').read_text(encoding='utf-8')))
