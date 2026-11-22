"""Create isolated development keys and sign S38 release manifests."""
import argparse
import csv
import hashlib
import json
import os
import re
import subprocess
import tempfile
from pathlib import Path
from urllib.parse import urlsplit

import momso_release_keys as keys
import momso_release_manifest as release


def run(*args):
    flags = getattr(subprocess, 'CREATE_NO_WINDOW', 0) if os.name == 'nt' else 0
    result = subprocess.run(args, stdin=subprocess.DEVNULL, stdout=subprocess.DEVNULL, stderr=subprocess.DEVNULL, check=False, creationflags=flags, timeout=60)
    if result.returncode: raise ValueError('signing_operation_failed')


def init_development(directory):
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
    private, public = target / 'dev-private.pem', target / 'dev-public.pem'
    openssl = release._shared._openssl()
    run(openssl, 'genpkey', '-algorithm', 'ED25519', '-out', str(private))
    os.chmod(private, 0o600)
    run(openssl, 'pkey', '-in', str(private), '-pubout', '-out', str(public))
    keys.validate('development', public)
    return public


def sign(bundle, server, web, bundle_url, image, sequence, version, private_key, public_key, output, channel):
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
    make = sub.add_parser('sign')
    for flag in ('bundle', 'server', 'web', 'private-key', 'public-key', 'output'):
        make.add_argument('--' + flag, type=Path, required=True)
    for flag in ('bundle-url', 'image', 'version'):
        make.add_argument('--' + flag, required=True)
    make.add_argument('--sequence', type=int, required=True)
    make.add_argument('--channel', choices=['production', 'development'], required=True)
    args = parser.parse_args()
    try:
        if args.action == 'init-development':
            init_development(args.directory)
        else:
            sign(args.bundle, args.server, args.web, args.bundle_url, args.image, args.sequence, args.version, args.private_key, args.public_key, args.output, args.channel)
        print('complete')
        return 0
    except (OSError, ValueError, subprocess.SubprocessError, release.ManifestError):
        print('signing_failed')
        return 1


if __name__ == '__main__': raise SystemExit(main())
