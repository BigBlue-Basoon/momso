import assert from 'node:assert/strict';
import { spawnSync } from 'node:child_process';
import test from 'node:test';
import { vmStartupScript } from './momso_anywhere_vm_startup.mjs';
const pythonBin = process.platform === 'win32' ? 'python' : 'python3';

test('VM startup validates the bundle, reads the secret privately, and publishes only the certificate fingerprint', () => {
  const source = vmStartupScript({ project: 'fixture-project', secret: 'momso-transport-aaaaaaaaaaaa', bundleUrl: 'https://example.test/release.tar.gz', bundleSha256: 'a'.repeat(64) });
  const python = source.split("<<'MOMSO_BOOTSTRAP'\n")[1].split('\nMOMSO_BOOTSTRAP')[0];
  const check = spawnSync(pythonBin, ['-c', 'import ast,sys; tree=ast.parse(sys.stdin.read()); units=[n.value.value for n in tree.body if isinstance(n,ast.Assign) and isinstance(n.targets[0],ast.Name) and n.targets[0].id=="unit"]; assert len(units)==2 and all("\\n[Service]\\n" in x for x in units)'], { input: python, encoding: 'utf8' });
  assert.equal(check.status, 0, check.stderr);
  assert(source.includes("hashlib.sha256(data).hexdigest()==cfg['bundleSha256']"));
  assert(source.includes('/versions/latest:access'));
  assert(source.includes('guest-attributes/momso/fingerprint'));
  assert(source.includes('Restart=on-failure'));
  assert(source.includes("('git','git')"));
  assert(source.includes('9af4d751fce183faedea629437d5252dbf458eeb04130cf4f97be844621ea985'));
  assert(!source.includes('fixture-oauth-token'));
});

test('VM startup rejects an unverified or non-HTTPS bundle', () => {
  assert.throws(() => vmStartupScript({ project: 'fixture-project', secret: 'momso-transport-aaaaaaaaaaaa', bundleUrl: 'http://example.test/a', bundleSha256: 'a'.repeat(64) }));
  assert.throws(() => vmStartupScript({ project: 'fixture-project', secret: 'momso-transport-aaaaaaaaaaaa', bundleUrl: 'https://example.test/a', bundleSha256: 'bad' }));
});

test('VM startup installs missing Git before starting the server', () => {
  const source = vmStartupScript({ project: 'fixture-project', secret: 'momso-transport-aaaaaaaaaaaa', bundleUrl: 'https://example.test/release.tar.gz', bundleSha256: 'a'.repeat(64) });
  const python = source.split("<<'MOMSO_BOOTSTRAP'\n")[1].split('\nMOMSO_BOOTSTRAP')[0];
  const check = String.raw`
import ast,sys
from types import SimpleNamespace
tree=ast.parse(sys.stdin.read())
assignment=next(n for n in tree.body if isinstance(n,ast.Assign) and any(isinstance(t,ast.Name) and t.id=='missing' for t in n.targets))
branch=next(n for n in tree.body if isinstance(n,ast.If) and isinstance(n.test,ast.Name) and n.test.id=='missing')
for available,expected in [({'bwrap','age-keygen','flock'},['git']),({'git','age-keygen','flock'},['bubblewrap']),(set(),['bubblewrap','git','age','util-linux']),({'bwrap','git','age-keygen','flock'},[])]:
 calls=[]; env={'shutil':SimpleNamespace(which=lambda name:name if name in available else None),'os':SimpleNamespace(environ={}),'run':lambda *args:calls.append(args)}
 exec(compile(ast.Module(body=[assignment,branch],type_ignores=[]),'fixture','exec'),env)
 assert env['missing']==expected
 assert (calls[-1][4:]==tuple(expected)) if expected else not calls
`;
  const result = spawnSync(pythonBin, ['-c', check], { input: python, encoding: 'utf8' });
  assert.equal(result.status, 0, result.stderr);
  assert(source.indexOf("('git','git')") < source.indexOf("run('systemctl','enable','--now','momso-server')"));
});

test('legacy Python accepts dot entries and rejects links or escaping paths', () => {
  const source = vmStartupScript({ project: 'fixture-project', secret: 'momso-transport-aaaaaaaaaaaa', bundleUrl: 'https://example.test/release.tar.gz', bundleSha256: 'a'.repeat(64) });
  const python = source.split("<<'MOMSO_BOOTSTRAP'\n")[1].split('\nMOMSO_BOOTSTRAP')[0];
  const check = String.raw`
import ast,io,pathlib,sys,tarfile,tempfile
from types import SimpleNamespace
source=sys.stdin.read()
tree=ast.parse(source,feature_version=(3,11))
block=next(node for node in ast.walk(tree) if isinstance(node,ast.Try)).body[0]
class LegacyTar:
 def __init__(self,**kwargs):self.tar=tarfile.open(**kwargs)
 def __enter__(self):return self
 def __exit__(self,*args):self.tar.close()
 def getmembers(self):return self.tar.getmembers()
 def extractall(self,path,members):self.tar.extractall(path,members=members)
def bundle(name,kind):
 out=io.BytesIO()
 with tarfile.open(fileobj=out,mode='w:gz') as tar:
  root=tarfile.TarInfo('./');root.type=tarfile.DIRTYPE;tar.addfile(root)
  item=tarfile.TarInfo(name);item.type=kind
  if kind in (tarfile.SYMTYPE,tarfile.LNKTYPE):item.linkname='elsewhere'
  if kind==tarfile.REGTYPE:item.size=2;tar.addfile(item,io.BytesIO(b'ok'))
  else:tar.addfile(item)
 return out.getvalue()
for name,kind,allowed in [('./web/index.html',tarfile.REGTYPE,True),('../escape',tarfile.REGTYPE,False),('/absolute',tarfile.REGTYPE,False),('./link',tarfile.SYMTYPE,False),('./hardlink',tarfile.LNKTYPE,False)]:
 with tempfile.TemporaryDirectory() as tmp:
  env={'tarfile':SimpleNamespace(open=lambda **kw:LegacyTar(**kw)),'io':io,'pathlib':pathlib,'data':bundle(name,kind),'tmp':tmp}
  try:exec(compile(ast.Module(body=[block],type_ignores=[]),'fixture','exec'),env)
  except AssertionError:
   if allowed:raise
  else:
   if not allowed:raise AssertionError('unsafe entry accepted')
   assert pathlib.Path(tmp,'web','index.html').read_bytes()==b'ok'
`;
  const result = spawnSync(pythonBin, ['-c', check], { input: python, encoding: 'utf8' });
  assert.equal(result.status, 0, result.stderr);
});

test('reboot preserves an already installed upgraded release', () => {
  const source = vmStartupScript({ project: 'fixture-project', secret: 'momso-transport-aaaaaaaaaaaa', bundleUrl: 'https://example.test/release.tar.gz', bundleSha256: 'a'.repeat(64) });
  const python = source.split("<<'MOMSO_BOOTSTRAP'\n")[1].split('\nMOMSO_BOOTSTRAP')[0];
  const check = String.raw`
import ast,os,pathlib,sys,tempfile
tree=ast.parse(sys.stdin.read())
guard=next(n for n in tree.body if isinstance(n,ast.If) and 'not installed' in ast.unparse(n.test))
with tempfile.TemporaryDirectory() as tmp:
 base=pathlib.Path(tmp);web=base/'web';web.mkdir()
 for name in ('momso-server','momso_mobile_proxy.py','momso_anywhere_vm_proxy.py','vault-provision.py','idle-stop.py','web/index.html','web/.momso-owned'):
  (base/name).write_text('new-version')
 installed=all((base/name).is_file() for name in ('momso-server','momso_mobile_proxy.py','momso_anywhere_vm_proxy.py','vault-provision.py','idle-stop.py','web/index.html','web/.momso-owned'))
 env={'os':os,'marker':str(base/'.bootstrap-release-installed'),'installed':installed}
 exec(compile(ast.Module(body=[guard],type_ignores=[]),'fixture','exec'),env)
 assert (base/'momso-server').read_text()=='new-version'
 assert (base/'web/index.html').read_text()=='new-version'
`;
  const result = spawnSync(pythonBin, ['-c', check], { input: python, encoding: 'utf8' });
  assert.equal(result.status, 0, result.stderr);
});

test('bootstrap does not replace an existing systemd unit', () => {
  const source = vmStartupScript({ project: 'fixture-project', secret: 'momso-transport-aaaaaaaaaaaa', bundleUrl: 'https://example.test/release.tar.gz', bundleSha256: 'a'.repeat(64) });
  const python = source.split("<<'MOMSO_BOOTSTRAP'\n")[1].split('\nMOMSO_BOOTSTRAP')[0];
  const check = String.raw`
import ast,pathlib,sys,tempfile
tree=ast.parse(sys.stdin.read())
guards=[n for n in tree.body if isinstance(n,ast.If) and ast.unparse(n.test)=='not os.path.exists(unit_path)']
assert len(guards)==2
with tempfile.TemporaryDirectory() as tmp:
 path=pathlib.Path(tmp,'service');path.write_text('upgraded unit')
 import os
 for guard in guards:exec(compile(ast.Module(body=[guard],type_ignores=[]),'fixture','exec'),{'os':os,'unit_path':str(path),'unit':'old unit'})
 assert path.read_text()=='upgraded unit'
`;
  const result = spawnSync(pythonBin, ['-c', check], { input: python, encoding: 'utf8' });
  assert.equal(result.status, 0, result.stderr);
});

test('vault credentials and idle service are installed without overwriting existing configuration', () => {
  const source = vmStartupScript({ project: 'fixture-project', secret: 'momso-transport-aaaaaaaaaaaa', bundleUrl: 'https://example.test/release.tar.gz', bundleSha256: 'a'.repeat(64) });
  const python = source.split("<<'MOMSO_BOOTSTRAP'\n")[1].split('\nMOMSO_BOOTSTRAP')[0];
  const check = String.raw`
import ast,os,pathlib,sys,tempfile
tree=ast.parse(sys.stdin.read())
helper=next(n.value.value for n in tree.body if isinstance(n,ast.Assign) and any(isinstance(t,ast.Name) and t.id=='helper' for t in n.targets))
ast.parse(helper)
idle=next(n.value.value for n in tree.body if isinstance(n,ast.Assign) and any(isinstance(t,ast.Name) and t.id=='idle' for t in n.targets))
assert ('$'+'{CREDENTIALS_DIRECTORY}/server-token') in idle
fn=next(n for n in tree.body if isinstance(n,ast.FunctionDef) and n.name=='write_once')
env={'os':os,'tempfile':tempfile};exec(compile(ast.Module(body=[fn],type_ignores=[]),'fixture','exec'),env)
with tempfile.TemporaryDirectory() as tmp:
 path=pathlib.Path(tmp,'unit');path.write_text('new configuration')
 env['write_once'](str(path),'old configuration')
 assert path.read_text()=='new configuration'
 created=pathlib.Path(tmp,'created');env['write_once'](str(created),'complete')
 assert created.read_text()=='complete'
 assert sorted(p.name for p in pathlib.Path(tmp).iterdir())==['created','unit']
`;
  const result = spawnSync(pythonBin, ['-c', check], { input: python, encoding: 'utf8' });
  assert.equal(result.status, 0, result.stderr);
  assert(source.includes('LoadCredential=momso-vault-identity:/run/momso-vault/momso-vault-identity'));
  assert(source.includes('momso-idle-stop.service'));
  assert(source.includes('RuntimeDirectoryPreserve=yes'));
  assert(source.includes('/usr/bin/flock -x /run/momso-vault/provision.lock'));
});
