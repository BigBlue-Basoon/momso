import base64
import datetime
import hashlib
import http.server
import io
import json
import subprocess
import sys
import tempfile
import tarfile
import threading
import unittest
from pathlib import Path

from release_manifest import _openssl, verify as verify_release, ManifestError
from trust_rotation import RootStore, TrustError, verify_rotation, current_release_key


class RotationTests(unittest.TestCase):
    def setUp(self):
        self.tmp = tempfile.TemporaryDirectory()
        self.addCleanup(self.tmp.cleanup)
        self.path = Path(self.tmp.name)
        self.keys = []
        for number in range(5):
            private = self.path / f'private-{number}'
            public = self.path / f'public-{number}'
            subprocess.run([_openssl(), 'genpkey', '-algorithm', 'ED25519', '-out', str(private)], check=True, capture_output=True)
            subprocess.run([_openssl(), 'pkey', '-in', str(private), '-pubout', '-outform', 'DER', '-out', str(public)], check=True, capture_output=True)
            self.keys.append((base64.b64encode(public.read_bytes()).decode(), private))
        self.now = datetime.datetime(2026, 11, 25, tzinfo=datetime.timezone.utc)

    def root(self, version, ids, release=0, expires='2030-01-01T00:00:00Z'):
        value = {'version': version, 'expires': expires, 'threshold': 2, 'keys': [self.keys[i][0] for i in ids], 'releaseKey': self.keys[release][0]}
        if version > 0: value['issuedAt'] = '2025-01-01T00:00:00Z'
        return json.dumps(value, separators=(',', ':')).encode()

    def signatures(self, raw, ids):
        source = self.path / 'root'
        source.write_bytes(raw)
        signatures = {}
        for i in ids:
            output = self.path / f'signature-{i}'
            subprocess.run([_openssl(), 'pkeyutl', '-sign', '-inkey', str(self.keys[i][1]), '-rawin', '-in', str(source), '-out', str(output)], check=True, capture_output=True)
            signatures[self.keys[i][0]] = output.read_bytes()
        return signatures

    def test_two_recovery_keys_revoke_compromised_online_key(self):
        old = self.root(1, [0, 1, 2])
        new = self.root(2, [1, 2, 3], release=3)
        result = verify_rotation(old, new, self.signatures(new, [1, 2]), now=self.now)
        self.assertNotIn(self.keys[0][0], result['keys'])
        self.assertEqual(result['releaseKey'], self.keys[3][0])

    def test_one_compromised_or_unknown_key_cannot_replace_trust(self):
        old = self.root(1, [0, 1, 2])
        new = self.root(2, [0, 1, 3])
        for ids in ([0], [0, 3], [3, 4]):
            with self.assertRaisesRegex(TrustError, 'signature_threshold_invalid'):
                verify_rotation(old, new, self.signatures(new, ids), now=self.now)

    def test_rollback_skipped_version_and_expiry_rejected(self):
        old = self.root(2, [0, 1, 2])
        for version in (1, 2, 4):
            new = self.root(version, [0, 1, 2])
            with self.assertRaisesRegex(TrustError, 'root_version_invalid'):
                verify_rotation(old, new, self.signatures(new, [0, 1]), now=self.now)
        expired = self.root(3, [0, 1, 2], expires='2026-01-01T00:00:00Z')
        verify_rotation(old, expired, self.signatures(expired, [0, 1]), now=self.now)
        with self.assertRaisesRegex(TrustError, 'root_expired'):
            current_release_key(expired, now=self.now)

    def test_new_root_must_also_authorize_rotation(self):
        old = self.root(1, [0, 1, 2])
        new = self.root(2, [2, 3, 4], release=3)
        with self.assertRaisesRegex(TrustError, 'signature_threshold_invalid'):
            verify_rotation(old, new, self.signatures(new, [0, 1]), now=self.now)
        self.assertEqual(verify_rotation(old, new, self.signatures(new, [0, 1, 3, 4]), now=self.now)['version'], 2)

    def test_shared_root_time_vectors(self):
        vectors_path = Path(__file__).resolve().parents[5] / 'scripts/fixtures/root-time-vectors.json'
        vectors = json.loads(vectors_path.read_text(encoding='utf-8'))
        now = datetime.datetime.strptime(vectors['now'], '%Y-%m-%dT%H:%M:%SZ').replace(tzinfo=datetime.timezone.utc)
        for case in vectors['cases']:
            with self.subTest(case=case['name']):
                def make(version, issued, expires):
                    value = json.loads(self.root(version, [0, 1, 2], expires=expires))
                    if issued is not None: value['issuedAt'] = issued
                    return json.dumps(value, separators=(',', ':')).encode()
                bootstrap = make(0, None, vectors['bootstrapExpires'])
                first = make(1, case['firstIssued'], case['firstExpires'])
                second = make(2, case['secondIssued'], case['secondExpires'])
                try:
                    verify_rotation(bootstrap, first, self.signatures(first, [0, 1]), now=now)
                    verify_rotation(first, second, self.signatures(second, [0, 1][:case['signers']]), now=now)
                    current_release_key(second, now=now)
                    accepted = True
                except TrustError:
                    accepted = False
                self.assertEqual(accepted, case['accepted'])

    def test_rotated_release_key_rejects_old_signature(self):
        new = self.root(2, [1, 2, 3], release=3)
        root = verify_rotation(self.root(1, [0, 1, 2]), new, self.signatures(new, [1, 2]), now=self.now)
        manifest = json.dumps({'sequence': 1, 'version': '0.1.1', 'bundleUrl': 'https://example.test/release', 'bundleSha256': 'a' * 64, 'gatewayImage': 'ghcr.io/example/gateway@sha256:' + 'b' * 64, 'channel': 'production'}).encode()
        public = self.path / 'active.pem'
        subprocess.run([_openssl(), 'pkey', '-in', str(self.keys[3][1]), '-pubout', '-out', str(public)], check=True, capture_output=True)
        signatures = self.signatures(manifest, [0, 3])
        with self.assertRaises(ManifestError): verify_release(manifest, signatures[self.keys[0][0]], public)
        self.assertEqual(root['releaseKey'], self.keys[3][0])
        self.assertEqual(verify_release(manifest, signatures[self.keys[3][0]], public)['sequence'], 1)

    @unittest.skipUnless(__import__('os').name == 'posix', 'VM state store requires POSIX locking')
    def test_state_survives_restart_and_rejects_rollback(self):
        old = self.root(1, [0, 1, 2])
        new = self.root(2, [1, 2, 3], release=3)
        directory = self.path / 'state'
        RootStore(directory, old).apply(new, self.signatures(new, [1, 2]), now=self.now)
        restarted = RootStore(directory, old)
        self.assertEqual(restarted.current(), new)
        child = subprocess.run(
            [sys.executable, '-B', '-c',
             'import base64,sys; from trust_rotation import RootStore; '
             'print(base64.b64encode(RootStore(sys.argv[1], base64.b64decode(sys.argv[2])).current()).decode())',
             str(directory), base64.b64encode(old).decode()],
            check=True, capture_output=True, text=True, cwd=Path(__file__).resolve().parent,
        )
        self.assertEqual(base64.b64decode(child.stdout.strip()), new)
        with self.assertRaisesRegex(TrustError, 'root_version_invalid'):
            restarted.apply(new, self.signatures(new, [1, 2]), now=self.now)

    @unittest.skipUnless(__import__('os').name == 'posix', 'VM updater requires POSIX locking')
    def test_installed_root_rotates_before_release_and_persists(self):
        from github_releases import ReleasePoller
        from test_update import Host
        from update import Updater, UpdateError
        bootstrap = self.root(0, [0, 1, 2])
        next_root = self.root(1, [1, 2, 3], release=3)
        directory = self.path / 'http'
        directory.mkdir()
        (directory / 'trust-root-1.json').write_bytes(next_root)
        signed = self.signatures(next_root, [1, 2])
        (directory / 'trust-root-1.sigs.json').write_text(json.dumps({k: base64.b64encode(v).decode() for k, v in signed.items()}))
        bundle_path = directory / 'bundle.tar.gz'
        with tarfile.open(bundle_path, 'w:gz') as archive:
            for name in ('momso-server', 'momso_mobile_proxy.py', 'momso_anywhere_vm_proxy.py', 'web/index.html'):
                data = name.encode(); item = tarfile.TarInfo(name); item.size = len(data); archive.addfile(item, io.BytesIO(data))
        handler = lambda *args, **kwargs: http.server.SimpleHTTPRequestHandler(*args, directory=str(directory), **kwargs)
        server = http.server.ThreadingHTTPServer(('127.0.0.1', 0), handler)
        thread = threading.Thread(target=server.serve_forever, daemon=True); thread.start()
        self.addCleanup(thread.join); self.addCleanup(server.server_close); self.addCleanup(server.shutdown)
        base = f'http://127.0.0.1:{server.server_port}'
        manifest = json.dumps({'sequence': 2, 'version': '0.1.1', 'bundleUrl': base + '/bundle.tar.gz', 'bundleSha256': hashlib.sha256(bundle_path.read_bytes()).hexdigest(), 'gatewayImage': 'registry.example/momso@sha256:' + 'b' * 64, 'channel': 'production'}).encode()
        (directory / 'release.json').write_bytes(manifest)
        (directory / 'release.json.sig').write_bytes(self.signatures(manifest, [3])[self.keys[3][0]])
        class Source:
            channel = 'production'
            rotation_assets = [(1, {'json': base + '/trust-root-1.json', 'sigs.json': base + '/trust-root-1.sigs.json'})]
            def discover(self): return base + '/release.json', base + '/release.json.sig'
        store = RootStore(self.path / 'state', bootstrap)
        host = Host()
        updater = Updater(self.path / 'updater', self.path / 'unused', host, allow_http_for_tests=True, trust_store=store)
        with self.assertRaisesRegex(UpdateError, 'signature_invalid'):
            updater.check(base + '/release.json', base + '/release.json.sig')
        ReleasePoller(Source(), updater)._apply_roots()
        subprocess.run([sys.executable, '-B', '-c',
                        'import base64,sys; from trust_rotation import RootStore; '
                        'assert RootStore(sys.argv[1], base64.b64decode(sys.argv[2])).current() == base64.b64decode(sys.argv[3])',
                        str(self.path / 'state'), base64.b64encode(bootstrap).decode(), base64.b64encode(next_root).decode()],
                       cwd=Path(__file__).resolve().parent, check=True, capture_output=True)
        restarted_store = RootStore(self.path / 'state', bootstrap)
        self.assertEqual(restarted_store.current(), next_root)
        updater = Updater(self.path / 'updater', self.path / 'unused', host, allow_http_for_tests=True, trust_store=restarted_store)
        self.assertEqual(ReleasePoller(Source(), updater).poll(), 'completed')
        self.assertEqual(host.sequence, 2)
        self.assertEqual(RootStore(self.path / 'state', bootstrap).current(), next_root)
        with self.assertRaisesRegex(TrustError, 'root_version_invalid'):
            store.apply(next_root, signed, now=self.now)


if __name__ == '__main__':
    unittest.main()
