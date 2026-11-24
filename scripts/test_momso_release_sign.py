import hashlib
import io
import json
import subprocess
import sys
import tarfile
import tempfile
import unittest
from pathlib import Path
from unittest.mock import patch

import momso_release_keys as keys
import momso_release_manifest as release
import momso_release_sign as signer
import momso_server_install as installer


class SignTests(unittest.TestCase):
    def test_registry_manifest_check_rejects_image_index(self):
        image = 'ghcr.io/example/gateway@sha256:' + 'a' * 64
        for media in ('application/vnd.oci.image.index.v1+json', 'application/vnd.docker.distribution.manifest.list.v2+json'):
            with patch.object(signer.subprocess, 'run', return_value=subprocess.CompletedProcess([], 0, json.dumps({'mediaType': media}).encode(), b'')):
                with self.assertRaisesRegex(ValueError, 'gateway_image_not_single_manifest'):
                    signer.verify_gateway_manifest(image)
        with patch.object(signer.subprocess, 'run', return_value=subprocess.CompletedProcess([], 0, b'{"mediaType":"application/vnd.oci.image.manifest.v1+json"}', b'')):
            signer.verify_gateway_manifest(image)

    def setUp(self):
        self.tmp = tempfile.TemporaryDirectory(); self.addCleanup(self.tmp.cleanup)
        self.root = Path(self.tmp.name)
        self.production_private = self.root / 'production-private.pem'
        self.production_public = self.root / 'production-public.pem'
        openssl = release._shared._openssl()
        signer.run(openssl, 'genpkey', '-algorithm', 'ED25519', '-out', str(self.production_private))
        signer.run(openssl, 'pkey', '-in', str(self.production_private), '-pubout', '-out', str(self.production_public))
        self.dev_dir = self.root / 'development'
        for name, value in [('PRODUCTION_KEY', self.production_public), ('DEVELOPMENT_KEY', self.dev_dir / 'dev-public.pem')]:
            item = patch.object(keys, name, value); item.start(); self.addCleanup(item.stop)
        signer.init_development(self.dev_dir)
        self.server = self.root / 'server'; self.server.write_bytes(b'fixture-server')
        self.web = self.root / 'web.tar.gz'
        self.bundle = self.root / 'bundle.tar.gz'
        with tarfile.open(self.web, 'w:gz') as archive:
            item = tarfile.TarInfo('index.html'); item.size = 7; archive.addfile(item, io.BytesIO(b'fixture'))
        with tarfile.open(self.bundle, 'w:gz') as archive:
            archive.add(self.server, arcname='momso-server')
            item = tarfile.TarInfo('web/index.html'); item.size = 7; archive.addfile(item, io.BytesIO(b'fixture'))
        self.image = 'registry.example.test/momso/gateway@sha256:' + 'a' * 64

    def create(self, channel, private, public, output):
        return signer.sign(self.bundle, self.server, self.web, 'https://fixture.test/bundle', self.image, 1, '0.1.1', private, public, output, channel)

    def test_development_key_and_signed_channel_do_not_cross_to_production(self):
        private, public = self.dev_dir / 'dev-private.pem', self.dev_dir / 'dev-public.pem'
        manifest, signature = self.create('development', private, public, self.root / 'dev.json')
        raw, sig = manifest.read_bytes(), signature.read_bytes()
        self.assertEqual(release.verify(raw, sig, public, channel='development')['channel'], 'development')
        with self.assertRaises(release.ManifestError): release.verify(raw, sig, public, channel='production')
        with self.assertRaises(release.ManifestError): release.verify(raw, sig, self.production_public, channel='production')
        with self.assertRaises(ValueError): self.create('production', private, public, self.root / 'forbidden.json')
        with self.assertRaises(FileExistsError): signer.init_development(self.dev_dir)

    def test_production_release_uses_pinned_key_and_rejects_overwrite(self):
        output = self.root / 'production.json'
        manifest, signature = self.create('production', self.production_private, self.production_public, output)
        raw, sig = manifest.read_bytes(), signature.read_bytes()
        self.assertEqual(release.verify(raw, sig, self.production_public)['channel'], 'production')
        with self.assertRaises(release.ManifestError): release.verify(raw, sig, self.production_public, channel='development')
        with self.assertRaisesRegex(ValueError, 'output_exists'):
            self.create('production', self.production_private, self.production_public, output)
        self.assertEqual(manifest.read_bytes(), raw)

    def test_development_signing_command_does_not_print_private_key(self):
        private, public = self.dev_dir / 'dev-private.pem', self.dev_dir / 'dev-public.pem'
        output = self.root / 'command.json'
        result = subprocess.run([sys.executable, 'scripts/momso_release_sign.py', 'sign', '--channel', 'development', '--bundle', str(self.bundle), '--server', str(self.server), '--web', str(self.web), '--bundle-url', 'https://fixture.test/bundle', '--image', self.image, '--sequence', '1', '--version', '0.1.1', '--private-key', str(private), '--public-key', str(public), '--output', str(output)], capture_output=True, text=True, check=False)
        self.assertEqual(result.returncode, 0, result.stderr)
        self.assertNotIn(private.read_text(), result.stdout + result.stderr)
        self.assertEqual(release.verify(output.read_bytes(), output.with_name(output.name + '.sig').read_bytes(), public, channel='development')['channel'], 'development')

    def test_changed_server_cannot_be_signed_for_an_existing_bundle(self):
        self.server.write_bytes(b'tampered')
        with self.assertRaisesRegex(release.ManifestError, 'artifact_mismatch'):
            self.create('development', self.dev_dir / 'dev-private.pem', self.dev_dir / 'dev-public.pem', self.root / 'invalid.json')
        self.assertFalse((self.root / 'invalid.json').exists())

    def test_encrypted_test_key_can_use_environment_password_without_output(self):
        encrypted, public = self.root / 'encrypted.pem', self.root / 'encrypted-public.pem'
        openssl = release._shared._openssl()
        with patch.dict('os.environ', {'MOMSO_SIGNING_PASSWORD': 'fixture-only-password'}):
            signer.run(openssl, 'genpkey', '-algorithm', 'ED25519', '-aes-256-cbc', '-pass', 'env:MOMSO_SIGNING_PASSWORD', '-out', str(encrypted))
            signer.run(openssl, 'pkey', '-in', str(encrypted), '-passin', 'env:MOMSO_SIGNING_PASSWORD', '-pubout', '-out', str(public))
            manifest, signature = self.create('development', encrypted, public, self.root / 'encrypted.json')
        self.assertEqual(release.verify(manifest.read_bytes(), signature.read_bytes(), public, channel='development')['channel'], 'development')

    def test_development_release_upgrades_existing_unsigned_installation(self):
        manifest, signature = self.create('development', self.dev_dir / 'dev-private.pem', self.dev_dir / 'dev-public.pem', self.root / 'dev.json')
        state = self.root / 'install.json'
        old = {'version': 1, 'id': 'a' * 12, 'account': 'fixture@example.test', 'project': 'fixture-project', 'steps': {}, 'anywhere': {'client_id': 'fixture.apps.googleusercontent.com', 'owner_sub': '12345', 'login_origin': 'https://login.example.test', 'url': 'https://fixture.run.app'}}
        state.write_text(json.dumps(old), encoding='utf-8')
        url = 'https://fixture.test/release.json'
        args = ['anywhere', '--account', old['account'], '--project', old['project'], '--state', str(state), '--artifact', str(self.server), '--sha256', hashlib.sha256(self.server.read_bytes()).hexdigest(), '--web-archive', str(self.web), '--web-sha256', hashlib.sha256(self.web.read_bytes()).hexdigest(), '--google-client-id', old['anywhere']['client_id'], '--owner-sub', '12345', '--login-origin', old['anywhere']['login_origin'], '--release-manifest-url', url, '--release-public-key', str(self.dev_dir / 'dev-public.pem'), '--release-channel', 'development']
        responses = {url: manifest.read_bytes(), url + '.sig': signature.read_bytes(), 'https://fixture.test/bundle': self.bundle.read_bytes()}
        class Provider:
            project, install_id, name, network = 'fixture-project', 'a' * 12, 'fixture-vm', 'fixture-net'
            def __init__(self): self.calls = []
            def run(self, *parts, **_kwargs):
                if parts[:3] == ('compute', 'instances', 'describe'):
                    return {'labels': {'momso-install-id': self.install_id}, 'status': 'RUNNING', 'networkInterfaces': [{'network': '/networks/fixture-net'}]}
                if parts[:3] == ('run', 'services', 'describe'): return {'status': {'url': 'https://fixture.run.app'}}
                raise AssertionError('unexpected remote operation')
            def upgrade_checkpoint(self, action): self.calls.append(('checkpoint', action))
            def upgrade_sequence(self, sequence, commit=False): self.calls.append(('sequence', sequence, commit))
            def upgrade_server(self, *args): self.calls.append(('server',))
            def install_web(self, *args): self.calls.append(('web',))
            def health(self): self.calls.append(('health',))
            def ensure_private_proxy(self): self.calls.append(('private_proxy',))
        provider = Provider()
        import momso_anywhere_install
        import momso_anywhere_public_assets
        with patch.object(release, 'download', side_effect=lambda uri, *a, **kw: responses[uri]), patch.object(installer, 'GoogleProvider', return_value=provider), patch.object(momso_anywhere_public_assets, 'from_web_archive', return_value={}), patch.object(momso_anywhere_install.GoogleAnywhere, '_objects', return_value=({'fixture': 1}, {'fixture': 1}, {'fixture': 1})), patch.object(momso_anywhere_install.GoogleAnywhere, 'upgrade_gateway') as upgrade, patch('momso_anywhere_vm_services.install') as install_services, patch('momso_anywhere_update_install.grant_gateway_update') as grant_update, patch('momso_anywhere_update_install.install') as install_update:
            self.assertEqual(installer.main(args), 0)
        upgrade.assert_called_once_with('https://fixture.run.app', self.image)
        install_services.assert_called_once_with(provider, allow_upgrade_gate=True)
        grant_update.assert_called_once_with(provider)
        install_update.assert_called_once_with(provider, 'development', self.dev_dir / 'dev-public.pem', url)
        self.assertIn(('private_proxy',), provider.calls)
        self.assertEqual(provider.calls, [('sequence', 1, False), ('checkpoint', 'begin'), ('server',), ('web',), ('health',), ('sequence', 1, True), ('checkpoint', 'finish'), ('private_proxy',)])
        self.assertEqual(json.loads(state.read_text())['anywhere']['release_channel'], 'development')

    def test_development_install_rejects_a_valid_production_release(self):
        manifest, signature = self.create('production', self.production_private, self.production_public, self.root / 'production.json')
        state = self.root / 'install.json'
        old = {'version': 1, 'id': 'a' * 12, 'account': 'fixture@example.test', 'project': 'fixture-project', 'steps': {}, 'anywhere': {'client_id': 'fixture.apps.googleusercontent.com', 'owner_sub': '12345', 'login_origin': 'https://login.example.test', 'url': 'https://fixture.run.app', 'release_channel': 'development'}}
        state.write_text(json.dumps(old), encoding='utf-8')
        original = state.read_bytes()
        url = 'https://fixture.test/release.json'
        args = ['anywhere', '--account', old['account'], '--project', old['project'], '--state', str(state), '--artifact', str(self.server), '--sha256', hashlib.sha256(self.server.read_bytes()).hexdigest(), '--web-archive', str(self.web), '--web-sha256', hashlib.sha256(self.web.read_bytes()).hexdigest(), '--google-client-id', old['anywhere']['client_id'], '--owner-sub', '12345', '--login-origin', old['anywhere']['login_origin'], '--release-manifest-url', url, '--release-public-key', str(self.production_public)]
        responses = {url: manifest.read_bytes(), url + '.sig': signature.read_bytes(), 'https://fixture.test/bundle': self.bundle.read_bytes()}
        import momso_anywhere_public_assets
        with patch.object(release, 'download', side_effect=lambda uri, *a, **kw: responses[uri]), patch.object(installer, 'GoogleProvider') as provider, patch.object(momso_anywhere_public_assets, 'from_web_archive', return_value={}):
            self.assertEqual(installer.main(args), 1)
        provider.assert_not_called()
        self.assertEqual(state.read_bytes(), original)


if __name__ == '__main__': unittest.main()
