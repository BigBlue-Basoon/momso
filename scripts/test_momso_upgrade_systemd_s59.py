"""Isolated real-systemd regression; run only on the development VM as root."""
import ast
import base64
import contextlib
import http.server
import hashlib
import json
import os
from pathlib import Path
import sqlite3
import subprocess
import tempfile
import threading
import uuid
from unittest.mock import patch

import momso_server_install as installer


class Handler(http.server.BaseHTTPRequestHandler):
    maintenance = False
    def log_message(self, *args): pass
    def reply(self, value):
        data = json.dumps(value).encode()
        self.send_response(200); self.send_header('Content-Length', str(len(data))); self.end_headers(); self.wfile.write(data)
    def do_GET(self): self.reply({'maintenance': self.maintenance, 'quiescent': self.maintenance, 'maintenanceReady': True})
    def do_POST(self):
        type(self).maintenance = self.path == '/maintenance'
        self.reply({'maintenance': self.maintenance})


def main():
    if os.geteuid() != 0: raise RuntimeError('development_vm_root_required')
    suffix = uuid.uuid4().hex[:12]
    unit = 'momso-s59-test-' + suffix
    unit_file = Path('/etc/systemd/system') / (unit + '.service')
    proxy_unit = unit + '-proxy'
    proxy_file = Path('/etc/systemd/system') / (proxy_unit + '.service')
    with tempfile.TemporaryDirectory(prefix='momso-s59-') as tmp:
        base = Path(tmp); os.chmod(base, 0o755)
        root = base / 'opt'; root.mkdir(); data = base / 'data'; data.mkdir()
        web = root / 'web'; web.mkdir(); (web / '.momso-owned').write_text('momso'); (web / 'index.html').write_text('old web')
        db = data / 'db.sqlite'; vault = data / 'vault.age'; vault.write_text('vault before')
        archive = data / 'archive'; archive.mkdir(); (archive / 'doc.md').write_text('archive before')
        with sqlite3.connect(db) as c: c.execute('create table version(value text)'); c.execute("insert into version values('old')")
        for path in (data, db, vault, archive, archive / 'doc.md'):
            os.chown(path, 65534, 65534)
        server = root / 'momso-server'; server.write_text('#!/bin/sh\nexit 0\n'); server.chmod(0o755)
        config = base / 'server.env'; config.write_text('MOMSO_SERVER_TOKEN=fixture\nMOMSO_SERVER_DATA_DIR=' + str(data) + '\n')
        httpd = http.server.ThreadingHTTPServer(('127.0.0.1', 0), Handler)
        thread = threading.Thread(target=httpd.serve_forever, daemon=True); thread.start()
        port = httpd.server_port
        unit_file.write_text('[Unit]\nDescription=isolated momso S59 test\n[Service]\nType=oneshot\nExecStart=' + str(server) + '\nRemainAfterExit=yes\n')
        proxy_file.write_text('[Unit]\nRequires=' + unit + '.service\nAfter=' + unit + '.service\n[Service]\nType=oneshot\nExecStart=/bin/true\nRemainAfterExit=yes\n')
        try:
            subprocess.run(['systemctl', 'daemon-reload'], check=True)
            subprocess.run(['systemctl', 'start', unit], check=True)
            subprocess.run(['systemctl', 'start', proxy_unit], check=True)
            with patch.object(installer.shutil, 'which', return_value='gcloud'):
                p = installer.GoogleProvider('fixture@example.test', 'fixture-project', '0123456789ab', base / 'key')
            def checkpoint(action):
                with patch.object(p, 'ssh', return_value=b'momso_checkpoint_ok') as ssh: p.upgrade_checkpoint(action)
                tree = ast.parse(ssh.call_args.args[0]); encoded = next(n.args[0].value for n in ast.walk(tree) if isinstance(n, ast.Call) and isinstance(n.func, ast.Attribute) and n.func.attr == 'b64decode')
                payload = base64.b64decode(encoded).decode().replace("root='/opt/momso'", 'root=' + repr(str(root))).replace("env='/etc/momso-server.env'", 'env=' + repr(str(config))).replace('127.0.0.1:8765', '127.0.0.1:' + str(port)).replace("'momso-server'", repr(unit))
                context = {}
                try:
                    exec(compile(payload, 'actual-installer-checkpoint', 'exec'), context)
                finally:
                    if 'lock' in context: os.close(context['lock'])
            artifact = base / 'server.bin'; artifact.write_bytes(b'new release')
            web_bundle = base / 'web.tar'; web_bundle.write_bytes(b'new screen')
            digest = hashlib.sha256(artifact.read_bytes()).hexdigest()
            web_digest = hashlib.sha256(web_bundle.read_bytes()).hexdigest()
            class Provider:
                install_id = '0123456789ab'
                name = 'fixture-vm'
                project = 'fixture-project'
                network = 'fixture-network'
                def run(self, *args, **kwargs):
                    if args[:3] == ('compute', 'instances', 'describe'):
                        return {'labels': {'momso-install-id': self.install_id}, 'status': 'RUNNING', 'networkInterfaces': [{'network': '/networks/' + self.network}]}
                    if args[:3] == ('run', 'services', 'describe'):
                        return {'status': {'url': 'https://fixture.invalid'}}
                    raise AssertionError(args)
                def upgrade_checkpoint(self, action): checkpoint(action)
                def ensure_private_proxy(self):
                    subprocess.run(['systemctl', 'start', proxy_unit], check=True)
                    subprocess.run(['systemctl', 'is-active', '--quiet', proxy_unit], check=True)
                def upgrade_server(self, *_):
                    subprocess.run(['systemctl', 'restart', unit], check=True)
                    with sqlite3.connect(db) as c: c.execute("update version set value='migrated'")
                    vault.write_text('vault after'); (archive / 'doc.md').write_text('archive after')
                def install_web(self, *_): (web / 'index.html').write_text('new web')
                def health(self):
                    with sqlite3.connect(db) as c: value = c.execute('select value from version').fetchone()[0]
                    if value == 'migrated': raise installer.InstallError('fixture health failure')
            import momso_anywhere_install
            import momso_anywhere_public_assets
            state = {'anywhere': {'client_id': 'client', 'owner_sub': 'owner', 'login_origin': 'https://login.invalid', 'url': 'https://fixture.invalid'}}
            with patch.object(momso_anywhere_public_assets, 'from_web_archive', return_value={}), patch.object(momso_anywhere_install.GoogleAnywhere, '_objects', return_value=({'name': 'owned'}, {'name': 'owned'}, {'name': 'owned'})):
                try:
                    installer.upgrade_anywhere(Provider(), state, artifact, digest, web_bundle, web_digest, 'client', 'owner', 'https://login.invalid')
                except installer.InstallError as error:
                    assert '되돌렸습니다' in str(error), str(error)
                else:
                    raise AssertionError('failed health check was not reported')
            with sqlite3.connect(db) as c: assert c.execute('select value from version').fetchone()[0] == 'old'
            assert vault.read_text() == 'vault before' and (archive / 'doc.md').read_text() == 'archive before'
            assert data.stat().st_uid == 65534 and db.stat().st_uid == 65534 and vault.stat().st_uid == 65534
            assert (web / 'index.html').read_text() == 'old web'
            assert not Handler.maintenance
            subprocess.run(['systemctl', 'is-active', '--quiet', proxy_unit], check=True)
            print('PASS isolated systemd migration failure restored DB, vault, archive and web')
        finally:
            subprocess.run(['systemctl', 'stop', proxy_unit], check=False, stdout=subprocess.DEVNULL, stderr=subprocess.DEVNULL)
            proxy_file.unlink(missing_ok=True)
            subprocess.run(['systemctl', 'stop', unit], check=False, stdout=subprocess.DEVNULL, stderr=subprocess.DEVNULL)
            unit_file.unlink(missing_ok=True)
            subprocess.run(['systemctl', 'daemon-reload'], check=False, stdout=subprocess.DEVNULL, stderr=subprocess.DEVNULL)
            httpd.shutdown(); httpd.server_close(); thread.join(timeout=5)


if __name__ == '__main__': main()
