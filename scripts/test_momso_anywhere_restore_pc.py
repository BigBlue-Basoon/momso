import unittest
import io
import json
from unittest.mock import patch

from momso_anywhere_restore_pc import inspect_restore, discover_preserved, discover_in_project
from momso_anywhere_restore_plan import RestorePlanError


class FakeProvider:
    install_id = 'a' * 12
    def __init__(self, missing=None):
        self.missing, self.calls = missing, []
    def run(self, *args, parse=False):
        self.calls.append(args)
        label = {'momso-install-id': self.install_id}
        if args[:3] == ('storage', 'buckets', 'list'): return [] if self.missing == 'bucket' else [{'name': 'momso-backup-' + self.install_id, 'labels': label}]
        if args[:2] == ('secrets', 'list'): return [] if self.missing == 'vault' else [{'name': 'projects/fixture/secrets/momso-vault-' + self.install_id, 'labels': label}]
        if args[:3] == ('kms', 'keyrings', 'list'): return [] if self.missing == 'kms' else [{'name': 'projects/fixture/locations/x/keyRings/momso-backup-' + self.install_id}]
        if args[:3] == ('kms', 'keys', 'list'): return [{'name': 'projects/fixture/locations/x/keyRings/momso-backup-' + self.install_id + '/cryptoKeys/backup', 'labels': label}]
        if args[:3] == ('secrets', 'versions', 'list'): return [{'name': 'projects/fixture/secrets/vault/versions/1', 'state': 'ENABLED'}]
        if args[:4] == ('kms', 'keys', 'versions', 'list'): return [{'state': 'ENABLED'}]
        if args[:3] == ('storage', 'objects', 'list'): return [{'name': 'gs://momso-backup-' + self.install_id + '/momso-backup/slot-0.enc', 'updated': '2026-11-20T01:02:03Z'}]
        raise AssertionError(args)


class RestoreTests(unittest.TestCase):
    def test_preview_reports_candidates_without_initializing_state(self):
        import momso_server_install as installer
        provider = FakeProvider()
        candidates = [{'installationId': provider.install_id, 'assets': 'complete', 'pcIdCompatible': True}]
        output = io.StringIO()
        with patch.object(installer, 'GoogleProvider', return_value=provider), patch('momso_anywhere_restore_pc.discover_in_project', return_value=candidates), patch('sys.stdout', output), patch.object(installer, 'initialize_state', side_effect=AssertionError('must not create state')):
            result = installer.main(['restore-preview', '--account', 'fixture@example.test', '--project', 'fixture-project', '--state', 'unused.json'])
        self.assertEqual(result, 0)
        self.assertTrue(json.loads(output.getvalue())['candidates'][0]['restoreAvailable'])

    def test_discovery_reads_all_resource_types_without_mutation(self):
        class DiscoveryProvider(FakeProvider):
            def run(self, *args, parse=False):
                if args[:3] == ('compute', 'instances', 'list'):
                    self.calls.append(args)
                    return []
                return super().run(*args, parse=parse)
        provider = DiscoveryProvider()
        self.assertEqual(discover_in_project(provider)[0]['installationId'], provider.install_id)
        self.assertTrue(all('list' in call for call in provider.calls))

    def test_discovery_lists_two_paused_installations_without_guessing(self):
        ids = ('a' * 12, 'b' * 24)
        buckets = [{'name': 'momso-backup-' + ident[:12], 'labels': {'momso-install-id': ident}} for ident in ids]
        secrets = [{'name': 'projects/fixture/secrets/momso-vault-' + ident[:12], 'labels': {'momso-install-id': ident}} for ident in ids]
        keys = [{'name': 'projects/fixture/keyRings/momso-backup-' + ident[:12] + '/cryptoKeys/backup', 'labels': {'momso-install-id': ident}} for ident in ids]
        result = discover_preserved(buckets=buckets, secrets=secrets, keys=keys, instances=[])
        self.assertEqual([item['installationId'] for item in result], list(ids))
        self.assertEqual([item['pcIdCompatible'] for item in result], [True, True])
        self.assertTrue(all(item['assets'] == 'complete' for item in result))

    def test_vm_with_same_full_owner_in_another_zone_is_not_paused(self):
        ident = 'a' * 24
        labels = {'momso-install-id': ident}
        buckets = [{'name': 'momso-backup-' + ident[:12], 'labels': labels}]
        secrets = [{'name': 'projects/fixture/secrets/momso-vault-' + ident[:12], 'labels': labels}]
        keys = [{'name': 'projects/fixture/keyRings/momso-backup-' + ident[:12] + '/cryptoKeys/backup', 'labels': labels}]
        instances = [{'name': 'renamed-vm', 'labels': labels}]
        self.assertEqual(discover_preserved(buckets=buckets, secrets=secrets, keys=keys, instances=instances), [])

    def test_discovery_skips_remnants_of_deleted_installations(self):
        # S180: after "delete everything" only the KMS key is left (the bucket is
        # soft-deleted and absent from `buckets list`, the vault secret is gone)
        # with every version DESTROY_SCHEDULED. That is not a paused install.
        paused, deleted, half, running, key_only = 'a' * 12, 'b' * 12, 'c' * 12, 'd' * 12, 'e' * 12
        label = lambda ident: {'momso-install-id': ident}
        class Provider:
            def __init__(self): self.calls = []
            def run(self, *args, parse=False):
                self.calls.append(args)
                if args[:3] == ('storage', 'buckets', 'list'):
                    return [{'name': 'momso-backup-' + i, 'labels': label(i)} for i in (paused, half, running)]
                if args[:2] == ('storage', 'ls'):
                    raise AssertionError('soft-deleted buckets are not needed to rule out a remnant')
                if args[:2] == ('secrets', 'list'):
                    return [{'name': 'projects/fixture/secrets/momso-vault-' + i, 'labels': label(i)} for i in (paused, running)]
                if args[:3] == ('kms', 'keyrings', 'list'):
                    return [{'name': 'projects/fixture/locations/x/keyRings/momso-backup-' + i} for i in (paused, deleted, half, running, key_only)]
                if args[:3] == ('kms', 'keys', 'list'):
                    ring = next(a for a in args if a.startswith('--keyring=')).removeprefix('--keyring=')
                    ident = ring.removeprefix('momso-backup-')
                    return [{'name': 'projects/fixture/locations/x/keyRings/' + ring + '/cryptoKeys/backup', 'labels': label(ident)}]
                if args[:4] == ('kms', 'keys', 'versions', 'list'):
                    ring = next(a for a in args if a.startswith('--keyring=')).removeprefix('--keyring=')
                    if ring == 'momso-backup-' + deleted:
                        return [{'name': 'v/1', 'state': 'DESTROY_SCHEDULED'}, {'name': 'v/2', 'state': 'DESTROYED'}]
                    return [{'name': 'v/1', 'state': 'ENABLED'}]
                if args[:3] == ('compute', 'instances', 'list'):
                    return [{'name': 'momso-' + running, 'labels': label(running)}]
                raise AssertionError(args)
        provider = Provider()
        result = discover_in_project(provider)
        self.assertEqual([(item['installationId'], item['assets']) for item in result],
                         [(paused, 'complete'), (half, 'incomplete'), (key_only, 'incomplete')])
        self.assertTrue(all('list' in call for call in provider.calls))

    def test_owned_backup_inspection_never_creates_or_changes_resources(self):
        provider = FakeProvider()
        self.assertEqual(inspect_restore(provider), {'restoreAvailable': True, 'lastBackupAt': '2026-11-20T01:02:03Z', 'requiresRestore': True})
        self.assertTrue(all('list' in call for call in provider.calls))

    def test_mobile_id_inspection_uses_short_resource_names_and_full_labels(self):
        provider = FakeProvider()
        provider.install_id = 'a' * 12 + 'b' * 12
        original = provider.run
        def run(*args, **kwargs):
            result = original(*args, **kwargs)
            if isinstance(result, list):
                return [{**item, 'name': item.get('name', '').replace(provider.install_id, provider.install_id[:12])} for item in result]
            return result
        provider.run = run
        self.assertTrue(inspect_restore(provider)['restoreAvailable'])

    def test_missing_asset_fails_closed_before_create(self):
        for missing in ('bucket', 'vault', 'kms'):
            with self.subTest(missing=missing):
                provider = FakeProvider(missing)
                with self.assertRaisesRegex(RestorePlanError, 'restore_assets_incomplete'):
                    inspect_restore(provider)
                self.assertTrue(all('list' in call for call in provider.calls))


if __name__ == '__main__': unittest.main()
