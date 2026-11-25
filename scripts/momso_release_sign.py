"""Create isolated development keys and sign S38 release manifests."""
import argparse
import base64
import csv
import datetime
import hashlib
import importlib.util
import json
import os
import re
import subprocess
import tempfile
import urllib.error
import urllib.parse
import urllib.request
from pathlib import Path
from urllib.parse import urlsplit

import momso_release_keys as keys
import momso_release_manifest as release


def run(*args):
    flags = getattr(subprocess, 'CREATE_NO_WINDOW', 0) if os.name == 'nt' else 0
    result = subprocess.run(args, stdin=subprocess.DEVNULL, stdout=subprocess.DEVNULL, stderr=subprocess.DEVNULL, check=False, creationflags=flags, timeout=60)
    if result.returncode: raise ValueError('signing_operation_failed')


def init_keypair(directory, channel):
    if channel not in ('production', 'development'): raise ValueError('channel_invalid')
    target = Path(directory).resolve()
    repo = Path(__file__).resolve().parent.parent
    if target == repo or repo in target.parents:
        raise ValueError('key_directory_inside_repository')
    target.parent.mkdir(parents=True, exist_ok=True)
    target.mkdir(mode=0o700, exist_ok=False)
    if os.name == 'nt':
        flags = getattr(subprocess, 'CREATE_NO_WINDOW', 0)
        identity = subprocess.run(['whoami', '/user', '/fo', 'csv', '/nh'], stdin=subprocess.DEVNULL, capture_output=True, text=True, check=True, creationflags=flags, timeout=30)
        sid = next(csv.reader(identity.stdout.splitlines()))[-1]
        if not re.fullmatch(r'S-1-(?:[0-9]+-)*[0-9]+', sid): raise ValueError('identity_unavailable')
        run('icacls', str(target), '/inheritance:r', '/grant:r', f'*{sid}:(OI)(CI)F', '/grant:r', '*S-1-5-18:(OI)(CI)F')
    prefix = 'dev' if channel == 'development' else 'production'
    private, public = target / f'{prefix}-private.pem', target / f'{prefix}-public.pem'
    openssl = release._shared._openssl()
    run(openssl, 'genpkey', '-algorithm', 'ED25519', '-out', str(private))
    os.chmod(private, 0o600)
    run(openssl, 'pkey', '-in', str(private), '-pubout', '-out', str(public))
    if channel == 'development': keys.validate('development', public)
    else: keys.public_der(public)
    return public


def init_development(directory):
    return init_keypair(directory, 'development')


def init_production(directory):
    target = Path(directory).resolve()
    repo = Path(__file__).resolve().parent.parent
    if target == repo or repo in target.parents:
        raise ValueError('key_directory_inside_repository')
    target.parent.mkdir(parents=True, exist_ok=True)
    target.mkdir(mode=0o700, exist_ok=False)
    return [init_keypair(target / name, 'production') for name in ('primary', 'recovery-1', 'recovery-2')]


def create_production_root(public_keys, expires, output):
    if len(public_keys) != 3: raise ValueError('root_invalid')
    encoded = [base64.b64encode(keys.public_der(path)).decode('ascii') for path in public_keys]
    issued = datetime.datetime.now(datetime.timezone.utc).strftime('%Y-%m-%dT%H:%M:%SZ')
    raw = json.dumps({'version': 0, 'issuedAt': issued, 'expires': expires, 'threshold': 2, 'keys': encoded, 'releaseKey': encoded[0]}, separators=(',', ':')).encode('utf-8')
    source = Path(__file__).resolve().parent.parent / 'apps/alpha/desktop/src-tauri/momso-server-update/trust_rotation.py'
    spec = importlib.util.spec_from_file_location('momso_root_validation', source)
    module = importlib.util.module_from_spec(spec); spec.loader.exec_module(module)
    try:
        module.current_release_key(raw)
    except module.TrustError:
        raise ValueError('root_invalid') from None
    with Path(output).open('xb') as file:
        file.write(raw)
    return output


def rotate_production_root(current_path, public_keys, private_keys, expires, output):
    if len(public_keys) != 3 or len(private_keys) != 2: raise ValueError('root_invalid')
    source = Path(__file__).resolve().parent.parent / 'apps/alpha/desktop/src-tauri/momso-server-update/trust_rotation.py'
    spec = importlib.util.spec_from_file_location('momso_root_rotation', source)
    trust = importlib.util.module_from_spec(spec); spec.loader.exec_module(trust)
    current_raw = Path(current_path).read_bytes()
    try: current = trust.parse_root(current_raw)
    except trust.TrustError: raise ValueError('root_invalid') from None
    encoded = [base64.b64encode(keys.public_der(path)).decode('ascii') for path in public_keys]
    issued = datetime.datetime.now(datetime.timezone.utc).strftime('%Y-%m-%dT%H:%M:%SZ')
    raw = json.dumps({'version': current['version'] + 1, 'issuedAt': issued, 'expires': expires, 'threshold': 2, 'keys': encoded, 'releaseKey': encoded[0]}, separators=(',', ':')).encode('utf-8')
    output = Path(output).resolve(); signatures_path = output.with_name(output.stem + '.sigs.json')
    if output.name != f"trust-root-{current['version'] + 1}.json" or output.exists() or signatures_path.exists(): raise ValueError('output_exists')
    openssl = release._shared._openssl()
    with tempfile.TemporaryDirectory(prefix='.momso-root-sign-', dir=output.parent) as directory:
        temporary = Path(directory); candidate = temporary / 'root'; candidate.write_bytes(raw)
        signatures = {}
        passin = ('-passin', 'env:MOMSO_SIGNING_PASSWORD') if 'MOMSO_SIGNING_PASSWORD' in os.environ else ()
        for number, private in enumerate(private_keys):
            public = temporary / f'public-{number}.pem'; signature = temporary / f'signature-{number}'
            run(openssl, 'pkey', '-in', str(private), *passin, '-pubout', '-out', str(public))
            identity = base64.b64encode(keys.public_der(public)).decode('ascii')
            if identity in signatures: raise ValueError('root_invalid')
            run(openssl, 'pkeyutl', '-sign', '-inkey', str(private), *passin, '-rawin', '-in', str(candidate), '-out', str(signature))
            signatures[identity] = signature.read_bytes()
        try: trust.verify_rotation(current_raw, raw, signatures)
        except trust.TrustError: raise ValueError('root_invalid') from None
        sig = temporary / 'signatures'; sig.write_text(json.dumps({key: base64.b64encode(value).decode('ascii') for key, value in signatures.items()}, separators=(',', ':')), encoding='utf-8')
        os.link(candidate, output)
        try: os.link(sig, signatures_path)
        except OSError:
            if os.path.samestat(output.stat(), candidate.stat()): output.unlink()
            raise
    return output, signatures_path


class RegistryError(ValueError):
    pass


class _NoRedirect(urllib.request.HTTPRedirectHandler):
    def redirect_request(self, req, fp, code, msg, headers, newurl):
        return None


def verify_gateway_manifest(image, *, opener=None):
    match = re.fullmatch(r'ghcr\.io/([a-z0-9]+(?:[._-][a-z0-9]+)*(?:/[a-z0-9]+(?:[._-][a-z0-9]+)*)+)@(sha256:[a-f0-9]{64})', image)
    if not match: raise RegistryError('gateway_image_invalid')
    name, digest = match.groups()
    opener = opener or urllib.request.build_opener(_NoRedirect())
    accept = ', '.join(('application/vnd.oci.image.index.v1+json', 'application/vnd.oci.image.manifest.v1+json',
                        'application/vnd.docker.distribution.manifest.list.v2+json', 'application/vnd.docker.distribution.manifest.v2+json'))
    try:
        query = urllib.parse.urlencode({'scope': f'repository:{name}:pull', 'service': 'ghcr.io'})
        with opener.open(urllib.request.Request('https://ghcr.io/token?' + query, headers={'Accept': 'application/json'}), timeout=30) as response:
            if response.status != 200: raise RegistryError('gateway_registry_private' if response.status in (401, 403) else 'gateway_registry_unavailable')
            token = json.loads(response.read(65537))['token']
        if not isinstance(token, str) or not token or len(token) > 65536 or not re.fullmatch(r'[A-Za-z0-9._~+/-]+=*', token):
            raise RegistryError('gateway_registry_response_invalid')
        request = urllib.request.Request(f'https://ghcr.io/v2/{name}/manifests/{digest}', headers={'Accept': accept, 'Authorization': 'Bearer ' + token})
        with opener.open(request, timeout=30) as response:
            if response.status != 200: raise RegistryError('gateway_registry_private' if response.status in (401, 403) else 'gateway_registry_unavailable')
            content_type = response.headers.get('Content-Type', '').split(';', 1)[0].strip().lower()
            actual = response.headers.get('Docker-Content-Digest', '')
            if content_type not in ('application/vnd.oci.image.manifest.v1+json', 'application/vnd.docker.distribution.manifest.v2+json'):
                raise RegistryError('gateway_image_not_single_manifest')
            if actual != digest: raise RegistryError('gateway_image_mismatch')
    except urllib.error.HTTPError as error:
        raise RegistryError('gateway_registry_private' if error.code in (401, 403) else 'gateway_registry_unavailable') from None
    except (urllib.error.URLError, TimeoutError, OSError):
        raise RegistryError('gateway_registry_unavailable') from None
    except (ValueError, KeyError, TypeError) as error:
        if isinstance(error, RegistryError): raise
        raise RegistryError('gateway_registry_response_invalid') from None


def sign(bundle, server, web, bundle_url, image, sequence, version, private_key, public_key, output, channel, active_root=None):
    output = Path(output).resolve()
    signature = output.with_name(output.name + '.sig')
    if output.exists() or signature.exists(): raise ValueError('output_exists')
    if Path(bundle).stat().st_size > 536870912: raise ValueError('bundle_too_large')
    raw = Path(bundle).read_bytes()
    digest = hashlib.sha256(raw).hexdigest()
    release.verify_inputs(raw, digest, server, web)
    url = urlsplit(bundle_url)
    if url.scheme != 'https' or not url.hostname or url.username or url.password or url.fragment:
        raise ValueError('invalid_bundle_url')
    if channel == 'production' and active_root is not None:
        source = Path(__file__).resolve().parent.parent / 'apps/alpha/desktop/src-tauri/momso-server-update/trust_rotation.py'
        spec = importlib.util.spec_from_file_location('momso_sign_root', source)
        trust = importlib.util.module_from_spec(spec); spec.loader.exec_module(trust)
        try:
            root = Path(active_root).read_bytes()
            trust.current_release_key(root)
            if base64.b64decode(trust.parse_root(root)['releaseKey'], validate=True) != keys.public_der(public_key): raise ValueError('key_mismatch')
        except trust.TrustError:
            raise ValueError('root_invalid') from None
    else:
        if active_root is not None: raise ValueError('channel_invalid')
        keys.validate(channel, public_key)
    output.parent.mkdir(parents=True, exist_ok=True)
    openssl = release._shared._openssl()
    with tempfile.TemporaryDirectory(prefix='.momso-sign-', dir=output.parent) as directory:
        temp = Path(directory)
        derived = temp / 'derived-public.pem'
        passin = ('-passin', 'env:MOMSO_SIGNING_PASSWORD') if 'MOMSO_SIGNING_PASSWORD' in os.environ else ()
        run(openssl, 'pkey', '-in', str(private_key), *passin, '-pubout', '-out', str(derived))
        if keys.public_der(derived) != keys.public_der(public_key): raise ValueError('key_mismatch')
        manifest = {'sequence': sequence, 'version': version, 'bundleUrl': bundle_url, 'bundleSha256': digest, 'gatewayImage': image, 'channel': channel}
        content = json.dumps(manifest, ensure_ascii=False, separators=(',', ':')).encode('utf-8')
        candidate, sig = temp / 'manifest', temp / 'signature'
        candidate.write_bytes(content)
        run(openssl, 'pkeyutl', '-sign', '-inkey', str(private_key), *passin, '-rawin', '-in', str(candidate), '-out', str(sig))
        release.verify(content, sig.read_bytes(), public_key, channel=channel)
        os.link(candidate, output)
        try:
            os.link(sig, signature)
        except OSError:
            if os.path.samestat(output.stat(), candidate.stat()): output.unlink()
            raise
    return output, signature


def main():
    parser = argparse.ArgumentParser(description='Prepare a signed momso release without printing keys')
    sub = parser.add_subparsers(dest='action', required=True)
    init = sub.add_parser('init-development')
    init.add_argument('--directory', type=Path, default=Path.home() / 'Documents/dev/momso-vm-install/dev-signing')
    production_init = sub.add_parser('init-production')
    production_init.add_argument('--directory', type=Path, default=Path.home() / 'momso-keys/production')
    replacement = sub.add_parser('init-replacement')
    replacement.add_argument('--directory', type=Path, required=True)
    root = sub.add_parser('create-production-root')
    root.add_argument('--public-key', type=Path, action='append', required=True)
    root.add_argument('--expires', required=True)
    root.add_argument('--output', type=Path, required=True)
    rotate = sub.add_parser('rotate-production-root')
    rotate.add_argument('--current', type=Path, required=True)
    rotate.add_argument('--public-key', type=Path, action='append', required=True)
    rotate.add_argument('--signing-key', type=Path, action='append', required=True)
    rotate.add_argument('--expires', required=True)
    rotate.add_argument('--output', type=Path, required=True)
    verify = sub.add_parser('verify-gateway')
    verify.add_argument('--image', required=True)
    make = sub.add_parser('sign')
    production = sub.add_parser('sign-production')
    for command in (make, production):
        for flag in ('bundle', 'server', 'web', 'private-key', 'public-key', 'output'):
            command.add_argument('--' + flag, type=Path, required=True)
        for flag in ('bundle-url', 'image', 'version'):
            command.add_argument('--' + flag, required=True)
        command.add_argument('--sequence', type=int, required=True)
    make.add_argument('--channel', choices=['production', 'development'], required=True)
    make.add_argument('--verify-gateway-registry', action='store_true')
    production.add_argument('--active-root', type=Path)
    args = parser.parse_args()
    try:
        if args.action == 'init-development':
            init_development(args.directory)
        elif args.action == 'init-production':
            init_production(args.directory)
            print('Move both recovery key folders to separate locations outside this computer.')
        elif args.action == 'init-replacement':
            init_keypair(args.directory, 'production')
        elif args.action == 'create-production-root':
            create_production_root(args.public_key, args.expires, args.output)
        elif args.action == 'rotate-production-root':
            rotate_production_root(args.current, args.public_key, args.signing_key, args.expires, args.output)
        elif args.action == 'verify-gateway':
            verify_gateway_manifest(args.image)
        else:
            if args.action == 'sign-production' or args.verify_gateway_registry:
                verify_gateway_manifest(args.image)
            sign(args.bundle, args.server, args.web, args.bundle_url, args.image, args.sequence, args.version, args.private_key, args.public_key, args.output, 'production' if args.action == 'sign-production' else args.channel, getattr(args, 'active_root', None))
        print('complete')
        return 0
    except RegistryError as error:
        print(str(error))
        return 1
    except (OSError, ValueError, subprocess.SubprocessError, release.ManifestError):
        print('signing_failed')
        return 1


if __name__ == '__main__': raise SystemExit(main())
