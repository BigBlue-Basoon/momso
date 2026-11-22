// This is public VM metadata: never include an OAuth token or proxy secret here.
export function vmStartupScript({ project, secret, bundleUrl, bundleSha256 }) {
  if (!/^[a-z][a-z0-9-]{4,28}[a-z0-9]$/.test(project) || !/^momso-transport-[a-f0-9]{12}$/.test(secret) || !/^https:\/\/[a-zA-Z0-9.-]+(?::443)?\//.test(bundleUrl) || !/^[a-f0-9]{64}$/.test(bundleSha256)) throw new Error('invalid_vm_release');
  const config = JSON.stringify({ project, secret, bundleUrl, bundleSha256 });
  return `#!/bin/sh
set -eu
install -d -m 0755 /opt/momso
cat > /opt/momso/bootstrap.py <<'MOMSO_BOOTSTRAP'
import base64,hashlib,io,json,os,pathlib,secrets,shutil,subprocess,tarfile,tempfile,urllib.request
cfg=json.loads(${JSON.stringify(config)})
def run(*args):subprocess.run(args,check=True,stdout=subprocess.DEVNULL,stderr=subprocess.DEVNULL)
def fetch(url,headers=None):
 with urllib.request.urlopen(urllib.request.Request(url,headers=headers or {}),timeout=120) as r:return r.read()
base='/opt/momso';os.makedirs(base,exist_ok=True)
codex=base+'/codex';codex_marker=base+'/.codex-sha256'
if not (os.path.isfile(codex) and os.path.isfile(codex_marker) and hashlib.sha256(open(codex,'rb').read()).hexdigest()==open(codex_marker).read().strip()):
 archive=fetch('https://github.com/openai/codex/releases/download/rust-v0.169.0/codex-x86_64-unknown-linux-musl.tar.gz')
 assert hashlib.sha256(archive).hexdigest()=='9af4d751fce183faedea629437d5252dbf458eeb04130cf4f97be844621ea985'
 with tarfile.open(fileobj=io.BytesIO(archive),mode='r:gz') as tf:
  entries=tf.getmembers();assert len(entries)==1 and entries[0].name=='codex-x86_64-unknown-linux-musl' and entries[0].isfile()
  with tf.extractfile(entries[0]) as src,open(codex+'.new','wb') as dst:shutil.copyfileobj(src,dst)
 os.chmod(codex+'.new',0o755);os.replace(codex+'.new',codex)
 with open(codex_marker,'w') as f:f.write(hashlib.sha256(open(codex,'rb').read()).hexdigest())
missing=[pkg for executable,pkg in (('bwrap','bubblewrap'),('git','git'),('age-keygen','age'),('flock','util-linux')) if not shutil.which(executable)]
if subprocess.run(['/usr/bin/python3','-c','import cryptography'],stdout=subprocess.DEVNULL,stderr=subprocess.DEVNULL).returncode:missing.append('python3-cryptography')
if missing:
 os.environ['DEBIAN_FRONTEND']='noninteractive';run('apt-get','update','-qq');run('apt-get','install','-y','-qq',*missing)
web=os.path.join(base,'web'); marker=base+'/.bootstrap-release-installed'
backup_files=('momso_anywhere_backup.py','momso_anywhere_backup_agent.py','momso_anywhere_backup_google.py','momso_anywhere_vault_identity.py')
installed=all(os.path.isfile(os.path.join(base,name)) for name in ('momso-server','momso_mobile_proxy.py','momso_anywhere_vm_proxy.py','vault-provision.py','idle-stop.py','web/index.html','web/.momso-owned'))
if os.path.exists(marker) and not installed:raise RuntimeError('installed_release_incomplete')
if not os.path.exists(marker) and not installed:
 data=fetch(cfg['bundleUrl']);assert hashlib.sha256(data).hexdigest()==cfg['bundleSha256']
 tmp=tempfile.mkdtemp(prefix='.release-',dir=base)
 try:
  with tarfile.open(fileobj=io.BytesIO(data),mode='r:gz') as tar:
   members=tar.getmembers()
   safe=[]
   for m in members:
    parts=pathlib.PurePosixPath(m.name).parts
    if not parts and m.isdir():continue
    assert parts and not m.name.startswith('/') and '..' not in parts and (m.isdir() or m.isfile())
    safe.append(m)
   tar.extractall(tmp,members=safe,**({'filter':'data'} if hasattr(tarfile,'data_filter') else {}))
  for name in ('momso-server','momso_mobile_proxy.py','momso_anywhere_vm_proxy.py','vault-provision.py','idle-stop.py','web/index.html')+backup_files:assert os.path.isfile(os.path.join(tmp,name))
  for name in ('momso-server','momso_mobile_proxy.py','momso_anywhere_vm_proxy.py','vault-provision.py','idle-stop.py')+backup_files:
   dst=os.path.join(base,name)
   if name in backup_files and os.path.lexists(dst):
    assert os.path.isfile(dst) and not os.path.islink(dst) and open(dst,'rb').read()==open(os.path.join(tmp,name),'rb').read();continue
   os.replace(os.path.join(tmp,name),dst);os.chmod(dst,0o755)
  old=web+'.old'
  if os.path.exists(web):assert os.path.isfile(web+'/.momso-owned')
  if os.path.exists(old):assert os.path.isfile(old+'/.momso-owned');shutil.rmtree(old)
  open(os.path.join(tmp,'web','.momso-owned'),'w').write('momso')
  for path,dirs,files in os.walk(os.path.join(tmp,'web')):
   os.chmod(path,0o755)
   for name in files:os.chmod(os.path.join(path,name),0o644)
  if os.path.exists(web):os.replace(web,old)
  os.replace(os.path.join(tmp,'web'),web)
  if os.path.exists(old):shutil.rmtree(old)
 finally:shutil.rmtree(tmp,ignore_errors=True)
if not os.path.exists(marker):
 with open(marker,'x') as f:f.write('installed\\n')
if not all(os.path.isfile(os.path.join(base,name)) and not os.path.islink(os.path.join(base,name)) for name in backup_files):
 data=fetch(cfg['bundleUrl']);assert hashlib.sha256(data).hexdigest()==cfg['bundleSha256']
 with tarfile.open(fileobj=io.BytesIO(data),mode='r:gz') as tar:
  for name in backup_files:
   dest=os.path.join(base,name)
   if os.path.lexists(dest):assert os.path.isfile(dest) and not os.path.islink(dest);continue
   member=tar.getmember(name);assert member.isfile() and member.size<=1048576
   fd,temporary=tempfile.mkstemp(prefix='.momso-backup-worker-',dir=base)
   try:
    with tar.extractfile(member) as src,os.fdopen(fd,'wb') as dst:shutil.copyfileobj(src,dst);dst.flush();os.fsync(dst.fileno())
    os.chmod(temporary,0o644);os.link(temporary,dest)
   finally:os.unlink(temporary)
if subprocess.run(['id','momso'],stdout=subprocess.DEVNULL,stderr=subprocess.DEVNULL).returncode:run('useradd','--system','--create-home','--home-dir','/var/lib/momso','--shell','/usr/sbin/nologin','momso')
private='/etc/momso-private';os.makedirs(private,mode=0o700,exist_ok=True);os.chmod(private,0o700)
metadata='http://metadata.google.internal/computeMetadata/v1/'
auth=json.loads(fetch(metadata+'instance/service-accounts/default/token',{'Metadata-Flavor':'Google'}))['access_token']
secret=json.loads(fetch('https://secretmanager.googleapis.com/v1/projects/'+cfg['project']+'/secrets/'+cfg['secret']+'/versions/latest:access',{'Authorization':'Bearer '+auth}))
transport=base64.b64decode(secret['payload']['data'],validate=True).decode('ascii').strip();assert transport and '\\n' not in transport
path=private+'/transport-token'
if os.path.exists(path):assert open(path).read().strip()==transport
else:
 fd=os.open(path,os.O_WRONLY|os.O_CREAT|os.O_EXCL,0o600)
 with os.fdopen(fd,'w') as f:f.write(transport)
cert=private+'/certificate.pem';key=private+'/key.pem'
if not os.path.exists(cert) and not os.path.exists(key):run('openssl','req','-x509','-newkey','rsa:3072','-nodes','-days','3650','-subj','/CN=momso-private','-keyout',key,'-out',cert)
os.chmod(key,0o600)
env='/etc/momso-server.env'
if not os.path.exists(env):
 fd=os.open(env,os.O_WRONLY|os.O_CREAT|os.O_EXCL,0o600)
 with os.fdopen(fd,'w') as f:f.write('MOMSO_SERVER_TOKEN='+secrets.token_urlsafe(32)+'\\nMOMSO_SERVER_DATA_DIR=/var/lib/momso\\nMOMSO_SERVER_LISTEN=127.0.0.1:8765\\nMOMSO_SERVER_WEB_DIR=/opt/momso/web\\nMOMSO_CODEX_PATH=/opt/momso/codex\\n')
elif not any(line.startswith('MOMSO_CODEX_PATH=') for line in open(env)):
 with open(env,'a') as f:f.write('MOMSO_CODEX_PATH=/opt/momso/codex\\n')
vault_secret='momso-vault-'+cfg['secret'].removeprefix('momso-transport-')
os.makedirs('/run/momso-vault',mode=0o700,exist_ok=True);os.chmod('/run/momso-vault',0o700)
run('/usr/bin/flock','-x','/run/momso-vault/provision.lock','/usr/bin/python3',base+'/vault-provision.py','--project',cfg['project'],'--secret',vault_secret,'--vault-dir','/var/lib/momso','--create-initial')
helper="""import os,tempfile
with open('/etc/momso-server.env') as f:token=next(line.split('=',1)[1].strip() for line in f if line.startswith('MOMSO_SERVER_TOKEN='))
assert token and chr(10) not in token
p='/run/momso-vault/server-token'
fd,tmp=tempfile.mkstemp(prefix='.server-token-',dir='/run/momso-vault')
try:
 with os.fdopen(fd,'w') as f:f.write(token);f.flush();os.fsync(f.fileno())
 os.replace(tmp,p)
finally:
 if os.path.exists(tmp):os.unlink(tmp)
"""
def write_once(path,text):
 if os.path.exists(path):return
 fd,tmp=tempfile.mkstemp(prefix='.momso-unit-',dir=os.path.dirname(path))
 try:
  with os.fdopen(fd,'w') as f:f.write(text);f.flush();os.fsync(f.fileno())
  try:os.link(tmp,path)
  except FileExistsError:pass
 finally:
  os.unlink(tmp)
write_once(base+'/idle-token.py',helper)
credential='[Unit]\\nDescription=momso vault credential\\nAfter=network-online.target\\nWants=network-online.target\\n[Service]\\nType=oneshot\\nRuntimeDirectory=momso-vault\\nRuntimeDirectoryMode=0700\\nRuntimeDirectoryPreserve=yes\\nRestart=on-failure\\nRestartSec=10\\nExecStart=/usr/bin/flock -x /run/momso-vault/provision.lock /usr/bin/python3 /opt/momso/vault-provision.py --project '+cfg['project']+' --secret '+vault_secret+' --vault-dir /var/lib/momso\\nExecStartPost=/usr/bin/python3 /opt/momso/idle-token.py\\nRemainAfterExit=yes\\n[Install]\\nWantedBy=multi-user.target\\n'
write_once('/etc/systemd/system/momso-vault-credential.service',credential)
os.makedirs('/etc/systemd/system/momso-server.service.d',exist_ok=True)
write_once('/etc/systemd/system/momso-server.service.d/vault.conf','[Unit]\\nRequires=momso-vault-credential.service\\nAfter=momso-vault-credential.service\\n[Service]\\nLoadCredential=momso-vault-identity:/run/momso-vault/momso-vault-identity\\n')
backup_id=cfg['secret'].removeprefix('momso-transport-')
backup_bucket='momso-backup-'+backup_id
backup_key='projects/'+cfg['project']+'/locations/asia-northeast3/keyRings/momso-backup-'+backup_id+'/cryptoKeys/backup'
backup_conf='/etc/systemd/system/momso-server.service.d/backup.conf'
backup_contents='[Service]\\nEnvironment=MOMSO_BACKUP_AGENT=/opt/momso/momso_anywhere_backup_agent.py\\nEnvironment=MOMSO_BACKUP_BUCKET='+backup_bucket+'\\nEnvironment=MOMSO_BACKUP_KMS_KEY='+backup_key+'\\n'
write_once(backup_conf,backup_contents)
assert not os.path.islink(backup_conf) and open(backup_conf).read()==backup_contents
idle='[Unit]\\nDescription=momso idle stop\\nRequires=momso-server.service momso-vault-credential.service\\nAfter=momso-server.service momso-vault-credential.service\\n[Service]\\nUser=momso\\nLoadCredential=server-token:/run/momso-vault/server-token\\nExecStart=/usr/bin/python3 /opt/momso/idle-stop.py --token-file \${CREDENTIALS_DIRECTORY}/server-token --state-dir /var/lib/momso/lifecycle\\nRestart=on-failure\\nRestartSec=60\\n[Install]\\nWantedBy=multi-user.target\\n'
write_once('/etc/systemd/system/momso-idle-stop.service',idle)
unit='[Unit]\\nDescription=momso server\\nAfter=network-online.target\\n[Service]\\nUser=momso\\nEnvironmentFile=/etc/momso-server.env\\nExecStart=/opt/momso/momso-server\\nRestart=on-failure\\n[Install]\\nWantedBy=multi-user.target\\n'
unit_path='/etc/systemd/system/momso-server.service'
if not os.path.exists(unit_path):
 with open(unit_path,'x') as f:f.write(unit)
unit='[Unit]\\nDescription=momso private proxy\\nAfter=momso-server.service\\nRequires=momso-server.service\\n[Service]\\nLoadCredential=server-env:/etc/momso-server.env\\nEnvironment=MOMSO_MOBILE_HOST=private.momso\\nEnvironment=MOMSO_PRIVATE_PROXY_TOKEN_FILE=/etc/momso-private/transport-token\\nEnvironment=MOMSO_PRIVATE_CERT=/etc/momso-private/certificate.pem\\nEnvironment=MOMSO_PRIVATE_KEY=/etc/momso-private/key.pem\\nExecStart=/usr/bin/python3 /opt/momso/momso_anywhere_vm_proxy.py\\nNoNewPrivileges=true\\nPrivateTmp=true\\nRestart=on-failure\\n[Install]\\nWantedBy=multi-user.target\\n'
unit_path='/etc/systemd/system/momso-private-proxy.service'
if not os.path.exists(unit_path):
 with open(unit_path,'x') as f:f.write(unit)
run('systemctl','daemon-reload');run('systemctl','enable','--now','momso-vault-credential');run('systemctl','enable','--now','momso-server');run('systemctl','restart','momso-server');run('systemctl','enable','--now','momso-private-proxy');run('systemctl','restart','momso-private-proxy');run('systemctl','enable','--now','momso-idle-stop')
import ssl,time,urllib.error
ctx=ssl._create_unverified_context();req=urllib.request.Request('https://127.0.0.1:8766/health',headers={'Authorization':'Bearer '+transport})
deadline=time.monotonic()+30
while True:
 try:
  with urllib.request.urlopen(req,context=ctx,timeout=3) as r:
   if json.load(r).get('state')=='ready':break
 except (OSError,urllib.error.URLError):pass
 if time.monotonic()>=deadline:raise RuntimeError('private_proxy_not_ready')
 time.sleep(1)
der=subprocess.check_output(['openssl','x509','-in',cert,'-outform','DER']);fp=hashlib.sha256(der).hexdigest()
req=urllib.request.Request(metadata+'instance/guest-attributes/momso/fingerprint',data=fp.encode(),headers={'Metadata-Flavor':'Google'},method='PUT')
with urllib.request.urlopen(req,timeout=10):pass
MOMSO_BOOTSTRAP
cat > /etc/systemd/system/momso-bootstrap.service <<'MOMSO_UNIT'
[Unit]
Description=momso installation
After=network-online.target
Wants=network-online.target
[Service]
Type=oneshot
ExecStart=/usr/bin/python3 /opt/momso/bootstrap.py
Restart=on-failure
RestartSec=30
RemainAfterExit=yes
[Install]
WantedBy=multi-user.target
MOMSO_UNIT
systemctl daemon-reload
systemctl enable --now momso-bootstrap
`;
}
