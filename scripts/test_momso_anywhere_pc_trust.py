import base64
import json
import tempfile
import unittest
import urllib.error
from pathlib import Path
from unittest.mock import patch

import momso_anywhere_pc_trust as pc
import momso_release_keys as keys
import momso_release_sign as signer


class PcTrustTests(unittest.TestCase):
    def test_rotation_before_release_and_state_survives_restart(self):
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            public = signer.init_production(root / 'keys')
            fourth = signer.init_keypair(root / 'new-key', 'production')
            encoded = [base64.b64encode(keys.public_der(path)).decode() for path in [*public, fourth]]
            bootstrap = json.dumps({'version': 0, 'expires': '2099-01-01T00:00:00Z', 'threshold': 2, 'keys': encoded[:3], 'releaseKey': encoded[0]}, separators=(',', ':')).encode()
            candidate = json.dumps({'version': 1, 'issuedAt': '2025-01-01T00:00:00Z', 'expires': '2099-01-01T00:00:00Z', 'threshold': 2, 'keys': encoded[1:], 'releaseKey': encoded[3]}, separators=(',', ':')).encode()
            source = root / 'candidate'; source.write_bytes(candidate)
            signatures = {}
            for index in (1, 2):
                output = root / f'sig-{index}'
                signer.run(signer.release._shared._openssl(), 'pkeyutl', '-sign', '-inkey', str(public[index].parent / 'production-private.pem'), '-rawin', '-in', str(source), '-out', str(output))
                signatures[encoded[index]] = base64.b64encode(output.read_bytes()).decode()
            signature_bytes = json.dumps(signatures).encode()
            state = root / 'install.json'
            manifest = 'https://fixture.test/releases/v1/release.json'
            def download(url, limit):
                if url.endswith('trust-root-1.json'): return candidate
                if url.endswith('trust-root-1.sigs.json'): return signature_bytes
                raise urllib.error.HTTPError(url, 404, '', {}, None)
            with patch.object(pc.release, 'download', side_effect=download):
                self.assertEqual(keys.public_der(fourth), keys.public_der(self._save(root, pc.active_key(bootstrap, state, manifest))))
                self.assertEqual(keys.public_der(fourth), keys.public_der(self._save(root, pc.active_key(bootstrap, state, manifest))))
            with self.assertRaisesRegex(pc.trust.TrustError, 'root_version_invalid'):
                pc.trust.RootStore(str(state) + '.trust', bootstrap).apply(candidate, {k: base64.b64decode(v) for k, v in signatures.items()})

    @staticmethod
    def _save(root, raw):
        path = root / 'active.pem'; path.write_bytes(raw); return path


if __name__ == '__main__': unittest.main()
