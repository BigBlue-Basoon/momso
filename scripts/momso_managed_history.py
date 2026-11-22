"""Read the release's generated legacy fingerprints without requiring Git."""
import json
from pathlib import Path
import re


def load():
    data = json.loads((Path(__file__).resolve().parent / 'momso_managed_history.json').read_text(encoding='utf-8'))
    if data.get('version') != 1:
        raise ValueError('invalid_managed_history')
    for group in ('files', 'worker', 'proxy'):
        values = data.get(group)
        if not isinstance(values, dict) or not values:
            raise ValueError('invalid_managed_history')
        for name, hashes in values.items():
            if not re.fullmatch(r'[a-z0-9_.-]{1,80}', name) or not isinstance(hashes, list) or not hashes or any(not isinstance(value, str) or not re.fullmatch(r'[0-9a-f]{64}', value) for value in hashes):
                raise ValueError('invalid_managed_history')
    return data
