"""Run mobile bootstrap in a disposable, network-isolated filesystem."""
import base64
import hashlib
import json
import os
from pathlib import Path
import subprocess
import tempfile
import unittest

import prepare_momso_anywhere_bundle


class UpdateBootstrapTests(unittest.TestCase):
    def test_mobile_bootstrap_installs_versioned_update_service(self):
        with tempfile.TemporaryDirectory(prefix="momso-s90-bootstrap-") as directory:
            root = Path(directory)
            server = root / "server"; server.write_text("fixture server")
            web = root / "web"; web.mkdir(); (web / "index.html").write_text("fixture web")
            bundle = root / "release.tar.gz"
            digest = prepare_momso_anywhere_bundle.prepare(server, hashlib.sha256(server.read_bytes()).hexdigest(), web, bundle)
            private, public = root / "key.pem", root / "public.der"
            subprocess.run(["openssl", "genpkey", "-algorithm", "ED25519", "-out", str(private)], check=True, capture_output=True)
            subprocess.run(["openssl", "pkey", "-in", str(private), "-pubout", "-outform", "DER", "-out", str(public)], check=True, capture_output=True)
            manifest = {"sequence": 9, "version": "0.1.9", "channel": "development", "bundleUrl": "https://fixture.test/bundle", "bundleSha256": digest, "gatewayImage": "registry.example/fixture@sha256:" + "1" * 64}
            (root / "release.json").write_text(json.dumps(manifest))
            subprocess.run(["openssl", "pkeyutl", "-sign", "-inkey", str(private), "-rawin", "-in", str(root / "release.json"), "-out", str(root / "release.json.sig")], check=True, capture_output=True)
            key = base64.b64encode(public.read_bytes()).decode()
            script = subprocess.check_output(["node", "--input-type=module", "-e", "import {vmStartupScript} from './scripts/momso_anywhere_vm_startup.mjs'; process.stdout.write(vmStartupScript({project:'fixture-project',secret:'momso-transport-aaaaaaaaaaaa',bundleUrl:'https://fixture.test/bundle',bundleSha256:process.argv[1],update:{manifestUrl:'https://fixture.test/release.json',publicKeySpki:process.argv[2],channel:'development'}}));", digest, key], text=True)
            (root / "startup.sh").write_text(script)
            (root / "opt").mkdir(); (root / "bin").mkdir()
            systemctl = root / "bin" / "systemctl"
            systemctl.write_text('#!/bin/sh\nif [ "$1" = enable ] && [ "$3" = momso-bootstrap ]; then exec /usr/bin/python3 /opt/momso/bootstrap.py; fi\n')
            systemctl.chmod(0o700)
            (root / "sitecustomize.py").write_text('''import base64,io,json,os,pathlib,shutil,subprocess,urllib.request
original_run=subprocess.run
def run(args,*a,**kw):
 if args[0] in ('id','systemctl','/usr/bin/flock','useradd','apt-get'):
  if tuple(args[:3])==('systemctl','enable','--now') and args[3]=='momso-vault-credential':
   pathlib.Path('/run/momso-vault').mkdir(parents=True,exist_ok=True)
   token=next(x.split('=',1)[1] for x in open('/etc/momso-server.env') if x.startswith('MOMSO_SERVER_TOKEN='))
   pathlib.Path('/run/momso-vault/server-token').write_text(token)
  return subprocess.CompletedProcess(args,0)
 return original_run(args,*a,**kw)
subprocess.run=run
original_which=shutil.which
shutil.which=lambda name,*a,**kw: '/usr/bin/true' if name=='gcloud' else original_which(name,*a,**kw)
state={'maintenance':False}
class Data(io.BytesIO):
 def __init__(self,path,url):super().__init__(pathlib.Path(path).read_bytes());self.url=url
 def geturl(self):return self.url
def urlopen(request,*a,**kw):
 url=request.full_url if hasattr(request,'full_url') else request
 if url=='https://fixture.test/bundle':return Data('/tmp/release.tar.gz',url)
 if url=='https://fixture.test/release.json':return Data('/tmp/release.json',url)
 if url=='https://fixture.test/release.json.sig':return Data('/tmp/release.json.sig',url)
 if url.endswith('/token'):return io.BytesIO(b'{"access_token":"fixture-token"}')
 if '/secrets/' in url:return io.BytesIO(json.dumps({'payload':{'data':base64.b64encode(b'fixture-transport-token').decode()}}).encode())
 if url.endswith('/health'):return io.BytesIO(b'{"state":"ready"}')
 if url.endswith('/activity'):return io.BytesIO(json.dumps({'maintenance':state['maintenance'],'quiescent':state['maintenance']}).encode())
 if url.endswith('/maintenance'):state['maintenance']=True;return io.BytesIO(b'{"maintenance":true}')
 if url.endswith('/maintenance/release'):state['maintenance']=False;return io.BytesIO(b'{"maintenance":false}')
 if '/guest-attributes/' in url:return io.BytesIO(b'{}')
 raise RuntimeError('unexpected fixture request')
urllib.request.urlopen=urlopen
class Opener:
 def open(self,request,timeout=None):return urlopen(request,timeout=timeout)
urllib.request.build_opener=lambda *args:Opener()
''')
            command = '''set -eu
mkdir -p /etc/systemd/system /etc/momso-private /opt/momso /var/lib/momso
printf fixture > /opt/momso/codex
sha256sum /opt/momso/codex | cut -d' ' -f1 > /opt/momso/.codex-sha256
sh /tmp/startup.sh
test -L /opt/momso/releases/current
test -f /opt/momso/releases/.momso-update-active
test -f /etc/systemd/system/momso-update.service
/usr/bin/python3 -c 'import json;assert json.load(open("/opt/momso/releases/installed.json"))["sequence"]==9;assert json.load(open("/etc/momso-update.json"))["channel"]=="development"'
'''
            subprocess.run(["bwrap", "--unshare-net", "--ro-bind", "/", "/", "--dev", "/dev", "--bind", str(root / "opt"), "/opt", "--tmpfs", "/etc", "--tmpfs", "/run", "--tmpfs", "/var", "--dir", "/etc/ssl", "--ro-bind", "/etc/ssl", "/etc/ssl", "--bind", str(root), "/tmp", "--setenv", "PYTHONPATH", "/tmp", "--setenv", "PATH", "/tmp/bin:/usr/bin:/bin", "/bin/sh", "-c", command], check=True, timeout=120)
