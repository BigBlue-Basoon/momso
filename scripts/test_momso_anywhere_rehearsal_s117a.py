import json
import io
import tempfile
import unittest
from pathlib import Path
from types import SimpleNamespace
from contextlib import contextmanager, redirect_stdout
from unittest.mock import patch

import momso_anywhere_rehearsal_s117a as rehearsal
from momso_anywhere_rehearsal_s117a import gateway_evidence, vm_evidence, inventory_basic_resources, inventory_regional_backup_keys
from momso_server_install import InstallError


class FakeProvider:
    install_id = 'a' * 12

    def __init__(self):
        self.calls = []
        self.traffic = [{'revisionName': 'momso-fixture-00001', 'percent': 100}]
        self.digest = 'sha256:' + '1' * 64
        self.vm = {'services': {name: 'active' for name in ('momso-server', 'momso-private-proxy', 'momso-idle-stop', 'momso-update')}, 'update_status': 'passed'}

    def run(self, *args, **kwargs):
        self.calls.append(args)
        if args[:3] == ('run', 'services', 'describe'):
            return {'status': {'traffic': self.traffic, 'url': 'https://fixture.run.app'}}
        if args[:3] == ('run', 'revisions', 'describe'):
            return {'status': {'imageDigest': self.digest, 'conditions': [{'type': 'Ready', 'status': 'True'}]}}
        raise AssertionError('unexpected operation')

    def ssh(self, script):
        self.calls.append(('ssh', script))
        return json.dumps(self.vm).encode()


@contextmanager
def fake_https(request, timeout):
    assert request.full_url == 'https://fixture.run.app/__momso/ready'
    assert timeout == 20
    class Response:
        status = 200
    yield Response()


class RehearsalReadOnlyTests(unittest.TestCase):
    def test_discovers_existing_names_and_fails_closed_on_unavailable_inventory(self):
        class Inventory:
            project = 'fake-project'
            def run(self, *args, **kwargs):
                if args[:3] == ('compute', 'instances', 'list'):
                    return [{'name': 'momso-bbbbbbbbbbbb'}]
                return b'fixture-token'
        class Opener:
            def open(self, request, timeout):
                return io.BytesIO(json.dumps({'items': [{'metadata': {'name': 'momso-any-cccccccccccc'}}]}).encode())
        with patch.object(rehearsal.urllib.request, 'build_opener', return_value=Opener()):
            self.assertEqual(rehearsal.discover_protected_installations(Inventory()), {'bbbbbbbbbbbb', 'cccccccccccc'})
        class Broken:
            def run(self, *args, **kwargs): raise RuntimeError('unavailable')
        with self.assertRaises(RuntimeError): rehearsal.discover_protected_installations(Broken())
    def test_receiving_revision_and_https_are_checked_independently(self):
        provider = FakeProvider()
        expected = 'ghcr.io/example/gateway@sha256:' + '1' * 64
        self.assertEqual(gateway_evidence(provider, expected, open_url=fake_https), {'traffic_digest': 'passed', 'public_https': 'passed'})
        provider.digest = 'sha256:' + '2' * 64
        self.assertEqual(gateway_evidence(provider, expected, open_url=fake_https), {'traffic_digest': 'unverified', 'public_https': 'passed'})
        provider.traffic = [{'revisionName': 'momso-fixture-00001', 'percent': 50}]
        self.assertEqual(gateway_evidence(provider, expected, open_url=fake_https)['traffic_digest'], 'unverified')
        self.assertTrue(all('delete' not in call for call in provider.calls))

    def test_vm_requires_four_active_services_and_authenticated_status(self):
        provider = FakeProvider()
        self.assertEqual(vm_evidence(provider), {'vm_services': 'passed', 'update_status': 'passed'})
        provider.vm['services']['momso-update'] = 'failed'
        provider.vm['update_status'] = 'unverified'
        self.assertEqual(vm_evidence(provider), {'vm_services': 'unverified', 'update_status': 'unverified'})

    def test_protected_installation_never_invokes_a_provider(self):
        provider = FakeProvider()
        provider.install_id = 'bbbbbbbbbbbb'
        provider.protected_installation_ids = {'bbbbbbbbbbbb'}
        with self.assertRaisesRegex(ValueError, 'invalid_installation_id'):
            gateway_evidence(provider, 'ghcr.io/example/gateway@sha256:' + '1' * 64, open_url=fake_https)
        with self.assertRaisesRegex(ValueError, 'invalid_installation_id'):
            vm_evidence(provider)
        self.assertEqual(provider.calls, [])

    def test_inventory_separates_empty_from_failed_and_nested_unknown(self):
        class InventoryProvider(FakeProvider):
            def run(self, *args, **kwargs):
                if args[:3] == ('compute', 'instances', 'list'):
                    return [{'name': 'momso-' + self.install_id, 'labels': {}}]
                if args[:3] == ('compute', 'disks', 'list'):
                    raise RuntimeError('token must not appear in output')
                if args[:3] == ('storage', 'buckets', 'describe'):
                    raise InstallError('absent', http_status=404)
                if args[:2] == ('storage', 'ls'): return b'[]'
                return []
        result = inventory_basic_resources(InventoryProvider())
        self.assertEqual(result['vm']['items'], ['momso-' + 'a' * 12])
        self.assertTrue(result['network']['complete'])
        self.assertFalse(result['disk']['complete'])
        self.assertTrue(result['gateway']['complete'])
        self.assertTrue(result['nat']['complete'])
        self.assertFalse(result['iam']['complete'])
        self.assertFalse(result['kms']['complete'])
        self.assertNotIn('token', repr(result))

    def test_nat_and_gateway_absent_present_and_query_failure(self):
        class InventoryProvider(FakeProvider):
            def __init__(self, state):
                super().__init__(); self.state = state
            def run(self, *args, **kwargs):
                if args[:3] == ('compute', 'routers', 'list'):
                    if self.state == 'failed': raise OSError('denied')
                    return [{'name': 'momso-' + self.install_id + '-router'}] if self.state in ('present', 'malformed') else []
                if args[:3] == ('compute', 'routers', 'describe'):
                    if self.state == 'malformed': return {'nats': []}
                    return {'name': 'momso-' + self.install_id + '-router', 'nats': [{'name': 'momso-' + self.install_id + '-nat'}]}
                if args[:3] == ('run', 'services', 'list'):
                    if self.state == 'failed': raise OSError('denied')
                    return [{'metadata': {'name': 'momso-any-' + self.install_id, 'labels': {}}}] if self.state == 'present' else []
                if args[:3] == ('storage', 'buckets', 'describe'):
                    raise InstallError('absent', http_status=404)
                if args[:2] == ('storage', 'ls'): return b'[]'
                return []
        for state in ('absent', 'present', 'failed'):
            result = inventory_basic_resources(InventoryProvider(state))
            for name in ('nat', 'gateway'):
                self.assertEqual(result[name]['complete'], state != 'failed')
                self.assertEqual(bool(result[name]['items']), state == 'present')
        self.assertFalse(inventory_basic_resources(InventoryProvider('malformed'))['nat']['complete'])

    def test_bucket_includes_soft_deleted_and_distinguishes_denied(self):
        class InventoryProvider(FakeProvider):
            def __init__(self, state): super().__init__(); self.state = state
            def run(self, *args, **kwargs):
                if args[:3] == ('storage', 'buckets', 'list'):
                    if self.state == 'denied': raise OSError('denied')
                    return [{'name': 'momso-backup-' + self.install_id}] if self.state == 'present' else []
                if args[:3] == ('storage', 'buckets', 'describe'):
                    if self.state == 'present':
                        return {'name': 'momso-backup-' + self.install_id, 'labels': {'momso-install-id': self.install_id}}
                    raise InstallError('denied' if self.state in ('denied', 'denied-description') else 'absent',
                                       http_status=403 if self.state in ('denied', 'denied-description') else 404)
                if args[:2] == ('storage', 'ls'):
                    if self.state == 'soft':
                        return json.dumps([{'metadata': {'name': 'momso-backup-' + self.install_id, 'hardDeleteTime': '2026-12-01T00:00:00Z'}}]).encode()
                    return b'[]'
                return []
        for state in ('absent', 'present', 'soft', 'denied', 'denied-description'):
            value = inventory_basic_resources(InventoryProvider(state))['bucket']
            self.assertEqual(value['complete'], state not in ('denied', 'denied-description'))
            self.assertEqual(bool(value['items']), state in ('present', 'soft'))
            if state == 'denied-description': self.assertEqual(value['reason'], 'bucket_access_denied')

    def test_iam_absent_present_and_query_failure(self):
        class InventoryProvider(FakeProvider):
            def run(self, *_args, **_kwargs): return []
        provider = InventoryProvider()
        for found in ([], ['project']):
            with patch('momso_anywhere_iam_pc.inspect_installation_bindings', return_value=found):
                result = inventory_basic_resources(provider)['iam']
                self.assertTrue(result['complete'])
                self.assertEqual(result['items'], found)
        with patch('momso_anywhere_iam_pc.inspect_installation_bindings', side_effect=OSError('denied')):
            result = inventory_basic_resources(provider)['iam']
            self.assertFalse(result['complete'])
            self.assertEqual(result['reason'], 'iam_inventory_failed')

    def test_scheduled_key_version_is_reported_and_missing_permission_is_unknown(self):
        class KeyProvider(FakeProvider):
            fail = False
            def run(self, *args, **kwargs):
                if args[:3] == ('kms', 'keyrings', 'list'):
                    return [{'name': 'projects/example/locations/region/keyRings/momso-backup-' + self.install_id}]
                if args[:3] == ('kms', 'keys', 'list'):
                    return [{'name': 'projects/example/locations/region/keyRings/ring/cryptoKeys/backup'}]
                if self.fail:
                    raise RuntimeError('permission denied')
                return [{'name': 'projects/example/locations/region/keyRings/ring/cryptoKeys/backup/cryptoKeyVersions/1', 'state': 'DESTROY_SCHEDULED', 'destroyTime': '2026-12-23T00:00:00Z'}]
        provider = KeyProvider()
        result = inventory_regional_backup_keys(provider)
        self.assertTrue(result['complete'])
        self.assertEqual(result['versions'][0]['state'], 'DESTROY_SCHEDULED')
        self.assertEqual(result['versions'][0]['destroyTime'], '2026-12-23T00:00:00Z')
        provider.fail = True
        self.assertFalse(inventory_regional_backup_keys(provider)['complete'])

    def test_fake_installer_failure_still_destroys_and_counts(self):
        calls = []
        def install():
            calls.append('install')
            raise RuntimeError('sensitive text')
        def destroy():
            calls.append('destroy')
            return True
        def backup():
            calls.append('backup')
            return 'backup_absent'
        def inventory():
            calls.append('inventory')
            return ({'vm': {'complete': True, 'items': []}}, [])
        result = rehearsal.run_rehearsal(FakeProvider(), install, lambda: {}, destroy, backup, inventory)
        self.assertEqual(calls, ['install', 'destroy', 'backup', 'inventory'])
        self.assertEqual(result['destroy'], 'passed')
        self.assertNotIn('sensitive text', repr(result))

    def test_found_owned_account_overrides_false_deleted_result(self):
        deletion = {'outcome': 'deleted_unverified'}
        remaining = {'accounts': {'complete': True, 'items': ['fixture-account']}}
        result = rehearsal.run_rehearsal(FakeProvider(), lambda: True, lambda: {}, lambda: deletion,
                                         lambda: 'backup_absent', lambda: (remaining, []))
        self.assertEqual(result['destroy'], 'failed')
        self.assertEqual(result['remainingKinds'], ['accounts'])

    def test_installer_failure_code_is_allowlisted_and_secrets_are_not_returned(self):
        process = SimpleNamespace(returncode=1, stdout=b'{"code":"gateway_image_mismatch","token":"SECRET"}\n')
        failure = rehearsal.installer_result(process, 'after_health')
        self.assertEqual(failure, {'ok': False, 'stage': 'after_health', 'reason': 'gateway_image_mismatch'})
        result = rehearsal.run_rehearsal(FakeProvider(), lambda: failure, lambda: {}, lambda: {'outcome': 'failed', 'reason': 'vm_unreachable'},
                                         lambda: 'backup_absent', lambda: ({}, []))
        self.assertEqual(result['installFailure']['reason'], 'gateway_image_mismatch')
        self.assertEqual(result['destroyFailure'], 'vm_unreachable')
        self.assertNotIn('SECRET', repr(result))
        self.assertEqual(rehearsal.installer_result(SimpleNamespace(returncode=1, stdout=b'{"code":"SECRET"}'), 'vm')['reason'], 'installer_failed')

    def test_proxy_failure_includes_safe_installer_diagnostic(self):
        process = SimpleNamespace(returncode=1, stdout=b'{"code":"proxy_not_running"}', stderr='실패: proxy_not_running\n'.encode())
        failure = rehearsal.installer_result(process, 'after_health')
        self.assertEqual(failure['reason'], 'proxy_not_running')
        self.assertEqual(failure['stderrTail'], '실패: proxy_not_running')
        process.stderr = b'failure\n\xec\x8b\xa4\xed\x8c\xa8: token=SECRET\n'
        self.assertNotIn('stderrTail', rehearsal.installer_result(process, 'after_health'))

    def test_failed_verification_and_exception_have_distinct_reasons(self):
        result = rehearsal.run_rehearsal(FakeProvider(), lambda: True, lambda: {'traffic_digest': 'unverified'}, lambda: True,
                                         lambda: 'backup_absent', lambda: ({}, []))
        self.assertEqual(result['checkFailures'], ['traffic_digest'])
        def failure(): raise ValueError('SECRET')
        result = rehearsal.run_rehearsal(FakeProvider(), lambda: True, failure, lambda: True,
                                         lambda: 'backup_absent', lambda: ({}, []))
        self.assertEqual(result['checkFailures'], ['verification_unavailable'])
        self.assertNotIn('SECRET', repr(result))

    def test_protected_id_stops_orchestration_before_any_callback(self):
        provider = FakeProvider()
        provider.install_id = 'bbbbbbbbbbbb'
        provider.protected_installation_ids = {'bbbbbbbbbbbb'}
        calls = []
        with self.assertRaises(ValueError):
            rehearsal.run_rehearsal(provider, *([lambda: calls.append(1)] * 5))
        self.assertEqual(calls, [])

    def test_preflight_rejects_cloud_run_collision_and_unavailable_listing(self):
        class PreflightProvider(FakeProvider):
            def __init__(self):
                super().__init__()
                self.collision = True
                self.failed = False
            def run(self, *args, **kwargs):
                if self.failed:
                    raise RuntimeError('unavailable')
                if args[:2] == ('storage', 'ls'):
                    return b'[]'
                if args[:3] == ('storage', 'buckets', 'describe'):
                    raise InstallError('absent', http_status=404)
                if args[:3] == ('run', 'services', 'list') and self.collision:
                    return [{'metadata': {'name': 'momso-any-' + self.install_id, 'labels': {}}}]
                return []
        provider = PreflightProvider()
        self.assertFalse(rehearsal.preflight_new_id(provider))
        provider.collision = False
        self.assertTrue(rehearsal.preflight_new_id(provider))
        provider.failed = True
        self.assertFalse(rehearsal.preflight_new_id(provider))

    def test_backup_cleanup_rejects_different_owner(self):
        from momso_anywhere_backup_provision import GoogleBackupProvisioner
        class BackupProvider(FakeProvider):
            project = 'fake-project'
            marker = 'momso-install-' + 'a' * 12
            def run(self, *args, **kwargs):
                self.calls.append(args)
                if args[:3] == ('storage', 'buckets', 'list'):
                    return [{'name': 'momso-backup-' + self.install_id}]
                if args[:3] == ('storage', 'buckets', 'describe'):
                    return {'labels': {'momso-install-id': 'b' * 12}}
                raise AssertionError('unexpected mutation')
        provider = BackupProvider()
        with self.assertRaises(Exception):
            GoogleBackupProvisioner(provider).destroy(delete_backup=True)
        self.assertFalse(any(call[:2] == ('storage', 'rm') for call in provider.calls))

    def test_key_cleanup_schedules_owned_version_only(self):
        class KeyProvider(FakeProvider):
            project = 'fake-project'
            owned = True
            def run(self, *args, **kwargs):
                self.calls.append(args)
                ring = f'projects/{self.project}/locations/{rehearsal.REGION}/keyRings/momso-backup-{self.install_id}'
                if args[:3] == ('kms', 'keyrings', 'list'):
                    return [{'name': ring}]
                if args[:3] == ('kms', 'keys', 'list'):
                    return [{'name': ring + '/cryptoKeys/backup', 'labels': {'momso-install-id': self.install_id if self.owned else 'b' * 12}}]
                if args[:4] == ('kms', 'keys', 'versions', 'list'):
                    return [{'name': ring + '/cryptoKeys/backup/cryptoKeyVersions/1', 'state': 'ENABLED'}]
                if args[:4] == ('kms', 'keys', 'versions', 'destroy'):
                    return b''
                raise AssertionError(args)
        provider = KeyProvider()
        provider.owned = False
        with self.assertRaises(ValueError):
            rehearsal.schedule_owned_backup_keys(provider)
        self.assertFalse(any(call[:4] == ('kms', 'keys', 'versions', 'destroy') for call in provider.calls))
        provider.owned = True
        self.assertEqual(rehearsal.schedule_owned_backup_keys(provider), 'keys_scheduled')
        self.assertEqual(sum(call[:4] == ('kms', 'keys', 'versions', 'destroy') for call in provider.calls), 1)

    def test_cli_forwards_google_fields_and_runs_fake_install_and_destroy(self):
        import momso_release_keys, momso_release_manifest, momso_anywhere_backup_provision
        class Provider(FakeProvider):
            def __init__(self, account, project, install_id, key_file):
                super().__init__()
                self.install_id, self.project = install_id, project
        class Backup:
            def __init__(self, provider): pass
            def destroy(self, *, delete_backup):
                assert delete_backup
                return 'backup_absent'
        calls = []
        class Completed:
            returncode = 0
            stdout = b'{"outcome":"deleted_unverified","unverified":["iam"]}'
        def fake_run(command, **kwargs):
            calls.append(command)
            return Completed()
        with tempfile.TemporaryDirectory() as tmp:
            bundle = Path(tmp)
            (bundle / 'momso-server').write_bytes(b'server')
            (bundle / 'web.tar.gz').write_bytes(b'web')
            key = bundle / 'key.pem'
            key.write_text('fake')
            argv = ['--account', 'fake@example.test', '--project', 'fake-project', '--release-manifest-url', 'https://example.test/release', '--bundle', str(bundle), '--public-key', str(key), '--channel', 'development', '--google-client-id', 'fake.apps.googleusercontent.com', '--owner-sub', '12345', '--login-origin', 'https://example.test', '--protect-installation-id', 'bbbbbbbbbbbb']
            signed = {'bundleUrl': 'https://example.test/bundle', 'bundleSha256': '0' * 64, 'gatewayImage': 'example/gateway@sha256:' + '1' * 64}
            checks = {'traffic_digest': 'passed', 'public_https': 'passed', 'vm_services': 'passed', 'update_status': 'passed'}
            names = ('vm', 'disk', 'network', 'subnet', 'firewall', 'router', 'nat', 'addresses', 'gateway', 'accounts', 'roles', 'secrets', 'iam', 'bucket', 'kms')
            inventory = {name: {'complete': True, 'items': []} for name in names}
            with patch.object(momso_release_keys, 'validate'), patch.object(momso_release_manifest, 'release', return_value=signed), patch.object(momso_release_manifest, 'download', return_value=b'fake'), patch.object(momso_release_manifest, 'verify_inputs'), patch.object(rehearsal, 'GoogleProvider', Provider), patch.object(rehearsal, 'preflight_new_id', return_value=True), patch.object(rehearsal, 'gateway_evidence', return_value=checks), patch.object(rehearsal, 'vm_evidence', return_value={}), patch.object(rehearsal, 'collect_inventory', return_value=(inventory, [])), patch.object(rehearsal, 'schedule_owned_backup_keys', return_value='keys_absent'), patch.object(rehearsal.subprocess, 'run', side_effect=fake_run), patch.object(momso_anywhere_backup_provision, 'GoogleBackupProvisioner', Backup), patch.object(rehearsal.tempfile, 'tempdir', tmp), redirect_stdout(io.StringIO()) as out:
                self.assertEqual(rehearsal.main(argv), 0)
            self.assertEqual([call[2] for call in calls], ['anywhere', 'destroy'])
            self.assertIn('--google-client-id', calls[0])
            self.assertIn('fake.apps.googleusercontent.com', calls[0])
            self.assertIn('--owner-sub', calls[0])
            self.assertIn('--login-origin', calls[0])
            self.assertNotIn('삭제 후 미확인: iam', out.getvalue())
            self.assertIn('과금되는 것: 확인한 범위에서 0', out.getvalue())

    def test_deletion_outcomes_distinguish_unverified_remaining_and_failure(self):
        for outcome, expected in [('deleted', 'passed'), ('deleted_unverified', 'passed'), ('remaining', 'failed'), ('failed', 'failed')]:
            deletion = {'outcome': outcome, 'unverified': ['iam'] if outcome == 'deleted_unverified' else []}
            result = rehearsal.run_rehearsal(FakeProvider(), lambda: True, lambda: {}, lambda: deletion,
                                             lambda: 'backup_absent,keys_absent', lambda: ({}, []))
            self.assertEqual(result['destroy'], expected)
            self.assertEqual(result['deletion'], deletion)

    def test_serverless_cleanup_reason_survives_rehearsal(self):
        deletion = {'outcome': 'remaining', 'reason': 'serverless_address_release_pending',
                    'retryCommand': 'fixture retry'}
        result = rehearsal.run_rehearsal(FakeProvider(), lambda: True, lambda: {}, lambda: deletion,
                                         lambda: 'backup_absent,keys_absent', lambda: ({}, []))
        self.assertEqual(result['destroyFailure'], 'serverless_address_release_pending')
        self.assertEqual(result['deletion']['retryCommand'], 'fixture retry')

    def test_final_iam_recheck_reconciles_unverified_outcome(self):
        for complete, items, expected in ((True, [], 'deleted'), (True, ['binding'], 'remaining'),
                                          (False, [], 'deleted_unverified')):
            deletion = {'outcome': 'deleted_unverified', 'unverified': ['iam_bindings']}
            groups = {'iam': {'complete': complete, 'items': items}}
            result = rehearsal.run_rehearsal(FakeProvider(), lambda: True, lambda: {}, lambda: deletion,
                lambda: 'backup_absent,keys_absent', lambda: (groups, []))
            self.assertEqual(result['deletion']['outcome'], expected)

    def test_final_inventory_can_identify_network_delay_after_backup_cleanup(self):
        provider = FakeProvider()
        provider.subnet, provider.network = 'fixture-sub', 'fixture-net'
        names = ('vm', 'disk', 'firewall', 'router', 'nat', 'gateway', 'accounts', 'roles', 'secrets', 'iam', 'bucket', 'kms', 'network', 'subnet', 'addresses')
        groups = {name: {'complete': True, 'items': []} for name in names}
        groups['subnet']['items'] = [provider.subnet]
        groups['network']['items'] = [provider.network]
        groups['addresses']['items'] = ['serverless-fixture']
        pending = {'reason': 'serverless_address_release_pending', 'addressCount': 1, 'billing': 'no_charge', 'retryable': True}
        with patch('momso_anywhere_network_cleanup.pending_serverless', return_value=pending):
            result = rehearsal.run_rehearsal(provider, lambda: True, lambda: {},
                lambda: {'outcome': 'remaining', 'reason': 'cleanup_pending'},
                lambda: 'backup_deleted,keys_absent', lambda: (groups, []),
                network_retry=lambda: {'retryCommand': 'fixture retry'})
        self.assertEqual(result['destroyFailure'], 'serverless_address_release_pending')
        self.assertEqual(result['deletion']['retryCommand'], 'fixture retry')

    def test_requested_idle_check_runs_before_destroy_and_failure_is_visible(self):
        calls = []
        checks = dict.fromkeys(('traffic_digest', 'public_https', 'vm_services', 'update_status'), 'passed')
        result = rehearsal.run_rehearsal(FakeProvider(), lambda: calls.append('install') or True,
            lambda: checks, lambda: calls.append('destroy') or True, lambda: 'backup_absent,keys_absent',
            lambda: ({}, []), idle=lambda _p: calls.append('idle') or {'status': 'failed', 'stage': 'waiting_for_idle_stop'})
        self.assertEqual(calls, ['install', 'idle', 'destroy'])
        self.assertEqual(result['idleWake']['stage'], 'waiting_for_idle_stop')


if __name__ == '__main__':
    unittest.main()
