"""Execute the generated VM bootstrap in a disposable bubblewrap filesystem."""
import hashlib
import io
import os
import subprocess
import tarfile
import tempfile
import unittest
from pathlib import Path
from momso_anywhere_proxy_files import PROXY_FILES


class BootstrapSandbox(unittest.TestCase):
    def test_generated_bootstrap_installs_verified_bundle(self):
        artifacts = Path(os.environ['MOMSO_S68_ARTIFACT_DIR'])
        expected = dict(line.split()[::-1] for line in (artifacts / 'SHA256SUMS').read_text().splitlines())
        for name in ('momso-server', 'web.tar.gz'):
            self.assertEqual(hashlib.sha256((artifacts / name).read_bytes()).hexdigest(), expected[name])
        with tempfile.TemporaryDirectory(prefix='momso-s68-bootstrap-') as directory:
            root = Path(directory)
            bundle = root / 'release.tar.gz'
            with tarfile.open(bundle, 'w:gz') as archive:
                archive.add(artifacts / 'momso-server', arcname='momso-server')
                for name in (*[item for item in PROXY_FILES if item != 'server_admission.py'],
                             'momso_anywhere_backup.py', 'momso_anywhere_backup_agent.py',
                             'momso_anywhere_backup_google.py', 'momso_anywhere_vault_identity.py'):
                    archive.add(Path('scripts') / name, arcname=name)
                archive.add(Path('apps/alpha/desktop/src-tauri/momso-server-update/server_admission.py'), arcname='server_admission.py')
                for name in ('vault-provision.py', 'idle-stop.py'):
                    content = b'#!/usr/bin/env python3\n'
                    item = tarfile.TarInfo(name)
                    item.size = len(content)
                    archive.addfile(item, io.BytesIO(content))
                with tarfile.open(artifacts / 'web.tar.gz', 'r:gz') as web:
                    for item in web.getmembers():
                        if not (item.isfile() or item.isdir()) or item.name.startswith('/') or '..' in Path(item.name).parts:
                            raise ValueError('unsafe test web archive')
                        if not Path(item.name).parts:
                            continue
                        source = web.extractfile(item) if item.isfile() else None
                        item.name = 'web/' + item.name.removeprefix('./')
                        archive.addfile(item, source)
            digest = hashlib.sha256(bundle.read_bytes()).hexdigest()
            script = subprocess.check_output(['node', '--input-type=module', '-e', "import {vmStartupScript} from './scripts/momso_anywhere_vm_startup.mjs'; process.stdout.write(vmStartupScript({project:'fixture-project',secret:'momso-transport-aaaaaaaaaaaa',bundleUrl:'https://fixture.test/release',bundleSha256:process.argv[1]}));", digest], text=True)
            (root / 'startup.sh').write_text(script)
            (root / 'opt').mkdir()
            (root / 'bin').mkdir()
            systemctl = root / 'bin' / 'systemctl'
            systemctl.write_text('#!/bin/sh\nif [ "$1" = enable ] && [ "$3" = momso-bootstrap ]; then exec /usr/bin/python3 /opt/momso/bootstrap.py; fi\n')
            systemctl.chmod(0o700)
            (root / 'sitecustomize.py').write_text('''import base64, io, json, subprocess, urllib.request
original_run=subprocess.run
def run(args,*a,**kw):
 if args[0] in ('id','systemctl','/usr/bin/flock','useradd','apt-get'):
  return subprocess.CompletedProcess(args,0)
 return original_run(args,*a,**kw)
subprocess.run=run
def urlopen(request,*a,**kw):
 url=request.full_url if hasattr(request,'full_url') else request
 if url=='https://fixture.test/release': return open('/tmp/release.tar.gz','rb')
 if url.endswith('/token'): return io.BytesIO(b'{"access_token":"fixture-token"}')
 if '/secrets/' in url: return io.BytesIO(json.dumps({'payload':{'data':base64.b64encode(b'fixture-transport-token').decode()}}).encode())
 if url.endswith('/health'): return io.BytesIO(b'{"state":"ready"}')
 if '/guest-attributes/' in url: return io.BytesIO(b'{}')
 raise RuntimeError('unexpected fixture request')
urllib.request.urlopen=urlopen
''')
            command = '''set -eu
mkdir -p /etc/systemd/system /etc/momso-private /opt/momso
printf fixture > /opt/momso/codex
sha256sum /opt/momso/codex | cut -d' ' -f1 > /opt/momso/.codex-sha256
sh /tmp/startup.sh
test -f /opt/momso/momso-server
test -f /opt/momso/web/index.html
test -f /opt/momso/.bootstrap-release-installed
test -f /etc/systemd/system/momso-server.service
'''
            subprocess.run(['bwrap', '--unshare-net', '--ro-bind', '/', '/', '--dev', '/dev', '--bind', str(root / 'opt'), '/opt', '--tmpfs', '/etc', '--tmpfs', '/run', '--tmpfs', '/var', '--dir', '/etc/ssl', '--ro-bind', '/etc/ssl', '/etc/ssl', '--bind', str(root), '/tmp', '--setenv', 'PYTHONPATH', '/tmp', '--setenv', 'PATH', '/tmp/bin:/usr/bin:/bin', '/bin/sh', '-c', command], check=True, timeout=120)
            if os.environ.get('MOMSO_S68_BOOTSTRAP_OUT'):
                import shutil
                shutil.copytree(root / 'opt' / 'momso', os.environ['MOMSO_S68_BOOTSTRAP_OUT'])


if __name__ == '__main__': unittest.main()
