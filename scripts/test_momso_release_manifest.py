import hashlib
import io
import json
import subprocess
import tarfile
import tempfile
import unittest
from pathlib import Path
from unittest.mock import patch

import momso_release_manifest as release
import momso_release_keys as keys
import momso_server_install as installer


class SignedReleaseTests(unittest.TestCase):
    def setUp(self):
        self.tmp = tempfile.TemporaryDirectory()
        self.addCleanup(self.tmp.cleanup)
        self.root = Path(self.tmp.name)
        self.private, self.public = self.root / 'private.pem', self.root / 'public.pem'
        subprocess.run(['openssl', 'genpkey', '-algorithm', 'ED25519', '-out', str(self.private)], check=True, capture_output=True)
        subprocess.run(['openssl', 'pkey', '-in', str(self.private), '-pubout', '-out', str(self.public)], check=True, capture_output=True)
        for name, value in [('PRODUCTION_KEY', self.public), ('DEVELOPMENT_KEY', self.root / 'absent-dev.pem')]:
            item = patch.object(keys, name, value); item.start(); self.addCleanup(item.stop)
        self.server = self.root / 'server'
        self.server.write_bytes(b'fixture server')
        self.web = self.root / 'web.tar.gz'
        with tarfile.open(self.web, 'w:gz') as archive:
            item = tarfile.TarInfo('index.html'); item.size = 7
            archive.addfile(item, io.BytesIO(b'fixture'))
        self.bundle = self.root / 'bundle.tar.gz'
        with tarfile.open(self.bundle, 'w:gz') as archive:
            archive.add(self.server, arcname='momso-server')
            item = tarfile.TarInfo('web/index.html'); item.size = 7
            archive.addfile(item, io.BytesIO(b'fixture'))
        self.image = 'registry.example.test/momso/gateway@sha256:' + 'a' * 64
        self.manifest = self.root / 'release.json'
        self.manifest.write_text(json.dumps({'sequence': 1, 'version': '0.1.1', 'bundleUrl': 'https://fixture.test/bundle', 'bundleSha256': hashlib.sha256(self.bundle.read_bytes()).hexdigest(), 'gatewayImage': self.image}), encoding='utf-8')
        self.signature = self.root / 'release.json.sig'
        subprocess.run(['openssl', 'pkeyutl', '-sign', '-inkey', str(self.private), '-rawin', '-in', str(self.manifest), '-out', str(self.signature)], check=True, capture_output=True)

    def test_same_bytes_are_verified_by_python_and_browser(self):
        raw, sig = self.manifest.read_bytes(), self.signature.read_bytes()
        info = release.verify(raw, sig, self.public)
        release.verify_inputs(self.bundle.read_bytes(), info['bundleSha256'], self.server, self.web)
        js = """import {webcrypto} from 'node:crypto'; import {readFileSync} from 'node:fs'; import {verifyRelease} from './scripts/momso_release_manifest.mjs'; const raw=readFileSync(process.argv[1]),sig=readFileSync(process.argv[2]),pem=readFileSync(process.argv[3],'utf8'),key=pem.replace(/-----[^-]+-----|\\s/g,''); const fetchImpl=async url=>({ok:true,arrayBuffer:async()=>{const b=url.endsWith('.sig')?sig:raw; return b.buffer.slice(b.byteOffset,b.byteOffset+b.byteLength)}}); const v=await verifyRelease({manifestUrl:'https://fixture.test/release.json',publicKeySpki:key,loginOrigin:'https://login.example.test'},{fetchImpl,cryptoImpl:webcrypto}); if(v.gatewayImage!==process.argv[4])process.exit(1);"""
        subprocess.run(['node', '--input-type=module', '-e', js, str(self.manifest), str(self.signature), str(self.public), self.image], check=True, capture_output=True)
        with self.assertRaisesRegex(release.ManifestError, 'signature_invalid'):
            release.verify(raw + b' ', sig, self.public)
        self.server.write_bytes(b'changed')
        with self.assertRaisesRegex(release.ManifestError, 'artifact_mismatch'):
            release.verify_inputs(self.bundle.read_bytes(), info['bundleSha256'], self.server, self.web)

    def test_bad_signature_stops_pc_installer_before_provider_or_state(self):
        url = 'https://fixture.test/release.json'
        args = ['anywhere', '--account', 'fixture@example.test', '--project', 'fixture-project', '--state', str(self.root / 'state.json'), '--artifact', str(self.server), '--sha256', hashlib.sha256(self.server.read_bytes()).hexdigest(), '--web-archive', str(self.web), '--web-sha256', hashlib.sha256(self.web.read_bytes()).hexdigest(), '--google-client-id', 'fixture.apps.googleusercontent.com', '--owner-sub', '12345', '--login-origin', 'https://login.example.test', '--release-manifest-url', url, '--release-public-key', str(self.public)]
        def fake_download(request, limit, **kwargs):
            del limit, kwargs
            if request.endswith('.sig'): return b'\x00' * 64
            if request == url: return self.manifest.read_bytes()
            raise AssertionError('bundle was downloaded before signature verification')
        with patch.object(release, 'download', side_effect=fake_download), patch.object(installer, 'GoogleProvider') as provider:
            self.assertEqual(installer.main(args), 1)
        provider.assert_not_called()
        self.assertFalse((self.root / 'state.json').exists())

    def test_valid_signature_passes_only_the_signed_image_to_the_installer(self):
        import momso_anywhere_install
        import momso_anywhere_public_assets
        url = 'https://fixture.test/release.json'
        args = ['anywhere', '--account', 'fixture@example.test', '--project', 'fixture-project', '--state', str(self.root / 'state.json'), '--artifact', str(self.server), '--sha256', hashlib.sha256(self.server.read_bytes()).hexdigest(), '--web-archive', str(self.web), '--web-sha256', hashlib.sha256(self.web.read_bytes()).hexdigest(), '--google-client-id', 'fixture.apps.googleusercontent.com', '--owner-sub', '12345', '--login-origin', 'https://login.example.test', '--release-manifest-url', url, '--release-public-key', str(self.public)]
        def fake_download(request, limit, **kwargs):
            del limit, kwargs
            return {url: self.manifest.read_bytes(), url + '.sig': self.signature.read_bytes(), 'https://fixture.test/bundle': self.bundle.read_bytes()}[request]
        class Provider:
            def prepare_network(self): pass
            def prepare_vm(self): pass
            def install(self, artifact, digest): pass
            def health(self): pass
            def install_web(self, archive, digest): pass
        with patch.object(release, 'download', side_effect=fake_download), patch.object(installer, 'GoogleProvider', return_value=Provider()), patch.object(momso_anywhere_public_assets, 'from_web_archive', return_value={}), patch.object(momso_anywhere_install, 'install', return_value='https://fixture.run.app') as install:
            self.assertEqual(installer.main(args), 0)
        self.assertEqual(install.call_args.args[-1], self.image)


if __name__ == '__main__': unittest.main()
