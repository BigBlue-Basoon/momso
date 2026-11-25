import base64
import hashlib
import io
import json
import subprocess
import tarfile
import tempfile
import unittest
import urllib.error
from pathlib import Path
from unittest.mock import patch

import momso_anywhere_pc_trust as pc
import momso_release_manifest as release
import momso_release_sign as signer


class CrossRuntimeTests(unittest.TestCase):
    def test_install_rotate_restart_and_use_new_release_in_pc_browser_and_vm(self):
        with tempfile.TemporaryDirectory() as directory:
            work = Path(directory)
            public = signer.init_production(work / 'keys')
            replacement = signer.init_keypair(work / 'new', 'production')
            bootstrap = work / 'bootstrap.json'
            signer.create_production_root(public, '2099-01-01T00:00:00Z', bootstrap)
            root, sigs = signer.rotate_production_root(bootstrap, [replacement, public[1], public[2]], [public[1].parent / 'production-private.pem', public[2].parent / 'production-private.pem'], '2099-01-01T00:00:00Z', work / 'trust-root-1.json')
            server = work / 'momso-server'; server.write_bytes(b'fixture server')
            web = work / 'web.tar.gz'
            with tarfile.open(web, 'w:gz') as archive:
                item = tarfile.TarInfo('index.html'); item.size = 7; archive.addfile(item, io.BytesIO(b'fixture'))
            bundle = work / 'bundle.tar.gz'
            with tarfile.open(bundle, 'w:gz') as archive:
                archive.add(server, arcname='momso-server')
                item = tarfile.TarInfo('web/index.html'); item.size = 7; archive.addfile(item, io.BytesIO(b'fixture'))
            manifest, signature = signer.sign(bundle, server, web, 'https://fixture.test/bundle.tar.gz', 'ghcr.io/example/gateway@sha256:' + 'a' * 64, 2, '0.1.1', replacement.parent / 'production-private.pem', replacement, work / 'release.json', 'production', root)
            url = 'https://fixture.test/release.json'
            def download(uri, limit):
                if uri.endswith('trust-root-1.json'): return root.read_bytes()
                if uri.endswith('trust-root-1.sigs.json'): return sigs.read_bytes()
                raise urllib.error.HTTPError(uri, 404, '', {}, None)
            with patch.object(pc.release, 'download', side_effect=download):
                active = pc.active_key(bootstrap.read_bytes(), work / 'pc-install.json', url)
            active_path = work / 'active.pem'; active_path.write_bytes(active)
            self.assertEqual(release.verify(manifest.read_bytes(), signature.read_bytes(), active_path)['sequence'], 2)
            vm = pc.trust.RootStore(work / 'vm-trust', bootstrap.read_bytes())
            signatures = {key: base64.b64decode(value) for key, value in json.loads(sigs.read_text()).items()}
            vm.apply(root.read_bytes(), signatures)
            restarted = pc.trust.RootStore(work / 'vm-trust', bootstrap.read_bytes())
            vm_key = work / 'vm-active.pem'; vm_key.write_bytes(pc.trust.current_release_key(restarted.current()))
            self.assertEqual(release.verify(manifest.read_bytes(), signature.read_bytes(), vm_key)['sequence'], 2)
            script = """import {webcrypto} from 'node:crypto'; import {readFileSync} from 'node:fs'; import {verifyRelease} from './scripts/momso_release_manifest.mjs'; const [base,root,sigs,manifest,signature,key]=process.argv.slice(1).map(p=>readFileSync(p)); const config={manifestUrl:'https://fixture.test/release.json',publicKeySpki:key.toString().replace(/-----[^-]+-----|\\s/g,''),trustBootstrap:base.toString('base64'),channel:'production'}; const state=new Map(); const storage={getItem:k=>state.get(k)??null,setItem:(k,v)=>state.set(k,v)}; const fetchImpl=async u=>{if(u.endsWith('trust-root-2.json'))return {ok:false,status:404};const b=u.endsWith('trust-root-1.sigs.json')?sigs:u.endsWith('trust-root-1.json')?root:u.endsWith('release.json.sig')?signature:manifest;return {ok:true,status:200,arrayBuffer:async()=>b.buffer.slice(b.byteOffset,b.byteOffset+b.byteLength)}}; const result=await verifyRelease(config,{fetchImpl,storage,cryptoImpl:webcrypto}); if(result.sequence!==2)process.exit(1);"""
            subprocess.run(['node', '--input-type=module', '-e', script, str(bootstrap), str(root), str(sigs), str(manifest), str(signature), str(public[0])], check=True, capture_output=True)


if __name__ == '__main__': unittest.main()
