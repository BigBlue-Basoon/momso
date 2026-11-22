import ast
import unittest
from pathlib import Path
from types import SimpleNamespace

from momso_iam_retry import bind_custom_role, role_propagation_failure
from momso_server_install import GoogleProvider, InstallError
from momso_anywhere_backup_provision import GoogleBackupProvisioner
from unittest.mock import patch


class IamRetryTests(unittest.TestCase):
    def test_known_custom_role_propagation_is_retried(self):
        class Provider:
            calls = 0
            def run(self, *args):
                self.calls += 1
                if self.calls < 3:
                    raise InstallError('safe message', role_propagating=True)
                return 'ok'
        provider, waits = Provider(), []
        self.assertEqual(bind_custom_role(provider, 'fixture', sleep=waits.append), 'ok')
        self.assertEqual((provider.calls, waits), (3, [5, 5]))

    def test_denied_and_other_errors_are_not_retried(self):
        class Provider:
            calls = 0
            def run(self, *args):
                self.calls += 1
                raise InstallError('permission denied')
        provider = Provider()
        with self.assertRaises(InstallError):
            bind_custom_role(provider, 'fixture', sleep=lambda _: self.fail('must not wait'))
        self.assertEqual(provider.calls, 1)

    def test_retry_limit_and_real_error_classification(self):
        text = b"HTTPError 400: Role (projects/fixture-project/roles/momsoBackup123) does not exist in the resource's hierarchy."
        self.assertTrue(role_propagation_failure(text))
        self.assertFalse(role_propagation_failure(b'HTTPError 403: Permission denied'))
        self.assertFalse(role_propagation_failure(b'Role (roles/viewer) does not exist in the resource hierarchy'))
        class Provider:
            calls = 0
            def run(self, *args):
                self.calls += 1
                raise InstallError('safe message', role_propagating=True)
        provider, waits = Provider(), []
        with self.assertRaises(InstallError):
            bind_custom_role(provider, 'fixture', sleep=waits.append)
        self.assertEqual((provider.calls, len(waits)), (12, 11))

    def test_provider_classifies_stderr_without_exposing_it(self):
        stderr = b"HTTPError 400: Role (projects/fixture-project/roles/momsoBackup123) does not exist in the resource's hierarchy."
        result = SimpleNamespace(returncode=1, stderr=stderr)
        with patch('momso_server_install.shutil.which', return_value='gcloud'), patch('momso_server_install.subprocess.run', return_value=result):
            provider = GoogleProvider('fixture', 'fixture-project', 'abcdef123456', Path('fixture'))
            with self.assertRaises(InstallError) as caught:
                provider.run('storage', 'buckets', 'add-iam-policy-binding')
        self.assertTrue(caught.exception.role_propagating)
        self.assertNotIn('fixture-project', str(caught.exception))

    def test_installer_policy_bindings_go_through_shared_helper(self):
        root = Path(__file__).resolve().parent
        for name in ('momso_wake_install.py', 'momso_anywhere_install.py',
                     'momso_anywhere_vm_services.py', 'momso_anywhere_backup_provision.py'):
            tree = ast.parse((root / name).read_text(encoding='utf-8'))
            for call in ast.walk(tree):
                if not isinstance(call, ast.Call) or not isinstance(call.func, ast.Attribute) or call.func.attr != 'run':
                    continue
                literals = [arg.value for arg in call.args if isinstance(arg, ast.Constant) and isinstance(arg.value, str)]
                self.assertNotIn('add-iam-policy-binding', literals, f'{name}:{call.lineno}')

    def test_backup_bucket_binding_retries_after_custom_role_creation(self):
        class Provider:
            install_id = 'abcdef123456'
            project = 'fixture-project'
            marker = 'momso-install-' + install_id
            attempts = 0
            def run(self, *args, **kwargs):
                if args[:3] == ('storage', 'buckets', 'add-iam-policy-binding'):
                    self.attempts += 1
                    if self.attempts < 3:
                        raise InstallError('safe message', role_propagating=True)
                return [] if kwargs.get('parse') else b''
        provider = Provider()
        with patch('momso_iam_retry.time.sleep') as sleep:
            GoogleBackupProvisioner(provider).provision()
        self.assertEqual(provider.attempts, 3)
        self.assertEqual(sleep.call_count, 2)


if __name__ == '__main__':
    unittest.main()
