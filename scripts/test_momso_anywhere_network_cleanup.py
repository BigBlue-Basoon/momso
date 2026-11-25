import unittest
from types import SimpleNamespace
from unittest.mock import patch
from momso_anywhere_network_cleanup import only_network_remains, pending_serverless, retry_network_only


class NetworkCleanupTests(unittest.TestCase):
    def provider(self, *, address=False, denied=False, foreign=False):
        ident = 'a' * 12
        p = SimpleNamespace(install_id=ident, name='momso-' + ident,
                            network='momso-' + ident + '-net', subnet='momso-' + ident + '-sub',
                            marker='momso-install-' + ident)
        live = {p.subnet, p.network}
        p.calls = []
        def run(*args, **kwargs):
            p.calls.append(args)
            if args[:3] == ('compute', 'addresses', 'list'):
                if denied: raise OSError('denied')
                return [{'name': 'serverless-fixture', 'purpose': 'SERVERLESS', 'status': 'RESERVED',
                         'addressType': 'INTERNAL', 'subnetwork': 'projects/example/regions/r/subnetworks/' + p.subnet}] if address else []
            if args[:3] in (('compute', 'instances', 'list'), ('run', 'services', 'list')): return []
            if 'list' in args:
                target = p.subnet if args[1:3] == ('networks', 'subnets') else p.network
                return [{'name': target, 'description': 'foreign' if foreign else p.marker}] if target in live else []
            if 'delete' in args:
                target = args[args.index('delete') + 1]
                live.remove(target)
                return b''
            raise AssertionError(args)
        p.run = run
        return p

    def test_waits_for_matching_internal_serverless_address(self):
        p = self.provider(address=True)
        remaining = retry_network_only(p)
        detail = pending_serverless(p, remaining)
        self.assertEqual(detail['reason'], 'serverless_address_release_pending')
        self.assertEqual(detail['billing'], 'no_charge')
        self.assertTrue(detail['retryable'])
        self.assertFalse(any('delete' in args for args in p.calls))

    def test_retry_deletes_only_remaining_owned_subnet_and_network(self):
        p = self.provider()
        self.assertEqual(retry_network_only(p), [])
        actions = [args for args in p.calls if 'delete' in args]
        self.assertEqual([args[1:3] for args in actions], [('networks', 'subnets'), ('networks', 'delete')])
        self.assertTrue(all('addresses' not in args for args in actions))

    def test_denied_and_foreign_are_not_treated_as_absent(self):
        with self.assertRaisesRegex(ValueError, 'address_inventory_failed'): retry_network_only(self.provider(denied=True))
        with self.assertRaisesRegex(ValueError, 'resource_ownership'): retry_network_only(self.provider(foreign=True))

    def test_hybrid_subnet_is_not_claimed_to_be_free(self):
        p = self.provider(address=True)
        run = p.run
        def hybrid(*args, **kwargs):
            rows = run(*args, **kwargs)
            if args[:4] == ('compute', 'networks', 'subnets', 'list'):
                for row in rows: row['purpose'] = 'HYBRID_SUBNET'
            return rows
        p.run = hybrid
        self.assertIsNone(pending_serverless(p, ['networks subnets:' + p.subnet, 'networks:' + p.network]))

    def test_other_reserved_address_prevents_network_only_verdict(self):
        p = self.provider(address=True)
        run = p.run
        def mixed(*args, **kwargs):
            rows = run(*args, **kwargs)
            if args[:3] == ('compute', 'addresses', 'list'):
                rows.append({'name': 'fixture-' + p.install_id, 'addressType': 'EXTERNAL', 'status': 'RESERVED'})
            return rows
        p.run = mixed
        self.assertIsNone(pending_serverless(p, ['networks subnets:' + p.subnet, 'networks:' + p.network]))

    def test_retry_refuses_malformed_or_surviving_gateway(self):
        p = self.provider()
        run = p.run
        def malformed(*args, **kwargs):
            if args[:3] == ('run', 'services', 'list'): return [{}]
            return run(*args, **kwargs)
        p.run = malformed
        with self.assertRaisesRegex(ValueError, 'inventory_unavailable'): retry_network_only(p)
        self.assertFalse(any('delete' in args for args in p.calls))
        def surviving(*args, **kwargs):
            if args[:3] == ('run', 'services', 'list'):
                return [{'name': 'projects/example/locations/r/services/momso-any-' + p.install_id}]
            return run(*args, **kwargs)
        p.run = surviving
        with self.assertRaisesRegex(ValueError, 'other_resources_present'): retry_network_only(p)

    def test_network_only_requires_other_inventory_to_be_complete_and_empty(self):
        names = ('vm', 'disk', 'firewall', 'router', 'nat', 'gateway', 'accounts', 'roles', 'secrets', 'iam', 'bucket', 'kms')
        groups = {name: {'complete': True, 'items': []} for name in names}
        with patch('momso_anywhere_rehearsal_s117a.collect_inventory', return_value=(groups, [])):
            self.assertTrue(only_network_remains(self.provider()))
        groups['bucket']['complete'] = False
        with patch('momso_anywhere_rehearsal_s117a.collect_inventory', return_value=(groups, [])):
            self.assertFalse(only_network_remains(self.provider()))
        groups['bucket'] = {'complete': True, 'items': ['retained']}
        with patch('momso_anywhere_rehearsal_s117a.collect_inventory', return_value=(groups, [])):
            self.assertFalse(only_network_remains(self.provider()))


if __name__ == '__main__': unittest.main()
