"""Files needed by the private proxy, shared by installers and updater."""
import json
from pathlib import Path

PROXY_FILES = tuple(json.loads(Path(__file__).with_suffix('.json').read_text(encoding='utf-8')))
