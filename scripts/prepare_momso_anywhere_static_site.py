"""Combine the existing login and start bundles for Cloudflare Pages."""
import argparse
import os
import tempfile
from pathlib import Path

import prepare_momso_anywhere_login_site as login
import prepare_momso_anywhere_start_site as start


HEADERS = """/*
  Cache-Control: no-store
  X-Content-Type-Options: nosniff
  Referrer-Policy: strict-origin
  X-Frame-Options: DENY
  Content-Security-Policy: default-src 'self'; script-src 'self'; style-src 'self' 'unsafe-inline'; img-src 'self' data:; connect-src 'self' https:; object-src 'none'; base-uri 'none'; frame-ancestors 'none'
"""


def prepare(*, output, login_page, web, start_dist, client_id, manifest_url, public_key, login_origin, trusted=()):
    output = Path(output).resolve()
    sources = [Path(p).resolve() for p in (login_page, web, start_dist)]
    if output.exists() or any(output == p or output in p.parents or p in output.parents for p in sources):
        raise ValueError('invalid_output')
    output.parent.mkdir(parents=True, exist_ok=True)
    with tempfile.TemporaryDirectory(prefix='.momso-static-', dir=output.parent) as temp:
        site = Path(temp) / 'site'
        login.prepare(site, client_id, login_page_dir=login_page, web=web)
        start.prepare(start_dist, site / 'start', client_id, manifest_url, public_key, login_origin, trusted, channel='production')
        if any((site / name).exists() for name in ('_headers', '_redirects', '404.html', '_worker.js', 'functions')):
            raise ValueError('site_overlap')
        (site / '_headers').write_text(HEADERS, encoding='utf-8')
        (site / '_redirects').write_text('/start /start/ 308\n', encoding='utf-8')
        (site / '404.html').write_text('<!doctype html><title>404</title>\n', encoding='utf-8')
        os.rename(site, output)


def main():
    parser = argparse.ArgumentParser()
    for name in ('output', 'login-page', 'web', 'start-dist', 'client-id', 'manifest-url', 'public-key', 'login-origin'):
        parser.add_argument('--' + name, required=True)
    parser.add_argument('--trusted-gateway-image', action='append', default=[])
    args = parser.parse_args()
    try:
        prepare(output=args.output, login_page=args.login_page, web=args.web, start_dist=args.start_dist,
                client_id=args.client_id, manifest_url=args.manifest_url, public_key=args.public_key,
                login_origin=args.login_origin, trusted=args.trusted_gateway_image)
    except (OSError, ValueError):
        print('static_site_not_ready')
        return 1
    return 0


if __name__ == '__main__':
    raise SystemExit(main())
