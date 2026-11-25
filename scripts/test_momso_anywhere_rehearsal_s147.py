import io
import json
from pathlib import Path
import subprocess
import sys
import tempfile
import unittest
from contextlib import redirect_stdout

import momso_anywhere_rehearsal_s147 as rehearsal


FAKE_GCLOUD = r'''import json,sys
from pathlib import Path
path=Path(sys.argv[1]); action=sys.argv[2]; target=sys.argv[3]
state=json.loads(path.read_text()); state['calls'].append([action,target]); path.write_text(json.dumps(state))
if action==state.get('fail'):
 print('fixture secret should never be included in result',file=sys.stderr);sys.exit(1)
if action=='write_marker':print('a'*64)
elif action=='read_marker':print(('b' if state.get('mismatch') else 'a')*64)
elif action=='check_paused':print(json.dumps({'vmAbsent':True,'gatewayAbsent':True,'backupPresent':True}))
elif action=='inventory':print(json.dumps({'complete':True,'remaining':[]}))
else:print('ok')
'''


class FakeGoogle:
    def __init__(self, root, **state):
        self.root = root
        self.program = root / 'fake_cloud.py'
        self.program.write_text(FAKE_GCLOUD, encoding='utf-8')
        self.state = root / 'state.json'
        self.state.write_text(json.dumps({'calls': [], **state}), encoding='utf-8')

    def _run(self, action, identity):
        result = subprocess.run([sys.executable, str(self.program), str(self.state), action, identity], capture_output=True, check=False, text=True)
        if result.returncode:
            raise rehearsal.RehearsalFailure('backup_unverified' if action == 'pause' else 'operation_failed')
        if action in ('check_paused', 'inventory'):
            return json.loads(result.stdout)
        if action in ('write_marker', 'read_marker'):
            return result.stdout.strip()
        return result.stdout.strip() == 'ok'

    def __getattr__(self, name):
        if name in ('preflight', 'install', 'write_marker', 'pause', 'check_paused', 'reinstall', 'read_marker', 'cleanup', 'inventory'):
            return lambda identity: self._run(name, identity)
        raise AttributeError(name)

    def calls(self):
        return json.loads(self.state.read_text(encoding='utf-8'))['calls']


class RehearsalTest(unittest.TestCase):
    def setUp(self):
        self.tmp = tempfile.TemporaryDirectory()
        self.addCleanup(self.tmp.cleanup)
        self.root = Path(self.tmp.name)
        self.identity = 'a' * 12
        self.protected = ['b' * 12]

    def execute(self, **options):
        fake = FakeGoogle(self.root, **options)
        return rehearsal.run_rehearsal(self.identity, self.protected, fake), fake

    def test_roundtrip_and_cleanup(self):
        result, fake = self.execute()
        self.assertEqual(result['outcome'], 'ok')
        self.assertEqual([call[0] for call in fake.calls()], ['preflight', 'install', 'write_marker', 'pause', 'check_paused', 'reinstall', 'read_marker', 'cleanup', 'inventory'])
        self.assertTrue(all(call[1] == self.identity for call in fake.calls()))

    def test_backup_failure_never_reinstalls_and_still_cleans(self):
        result, fake = self.execute(fail='pause')
        self.assertEqual(result['reason'], 'backup_unverified')
        self.assertNotIn('reinstall', [call[0] for call in fake.calls()])
        self.assertIn('cleanup', [call[0] for call in fake.calls()])
        self.assertNotIn('fixture secret', repr(result))

    def test_digest_mismatch_and_interrupted_install_are_cleaned(self):
        for options, reason in (({'mismatch': True}, 'digest_mismatch'), ({'fail': 'install'}, 'operation_failed')):
            with self.subTest(options=options):
                result, fake = self.execute(**options)
                self.assertEqual(result['reason'], reason)
                self.assertIn('cleanup', [call[0] for call in fake.calls()])

    def test_protected_id_never_invokes_cloud(self):
        fake = FakeGoogle(self.root)
        with self.assertRaises(rehearsal.RehearsalFailure):
            rehearsal.run_rehearsal(self.identity, [self.identity], fake)
        self.assertEqual(fake.calls(), [])

    def test_cleanup_failure_and_unknown_inventory_are_not_success(self):
        result, fake = self.execute(fail='cleanup')
        self.assertEqual(result['outcome'], 'failed')
        self.assertEqual(result['steps']['cleanup']['status'], 'failed')
        self.assertIn('inventory', [call[0] for call in fake.calls()])
        result, _ = self.execute(fail='inventory')
        self.assertEqual(result['outcome'], 'failed')
        self.assertEqual(result['leftovers']['status'], 'unverified')

    def test_cli_fails_before_any_cloud_action_until_restore_is_integrated(self):
        argv = []
        for name in ('account', 'project', 'release-manifest-url', 'bundle', 'public-key', 'channel', 'google-client-id', 'owner-sub', 'login-origin'):
            argv.extend(['--' + name, 'fixture'])
        with redirect_stdout(io.StringIO()) as output:
            self.assertEqual(rehearsal.main(argv), 1)
        result = json.loads(output.getvalue().splitlines()[-1])
        self.assertEqual(result['steps']['preflight']['reason'], 'restore_not_integrated')
        self.assertEqual(result['leftovers']['status'], 'not_created')


if __name__ == '__main__': unittest.main()
