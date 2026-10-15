#!/usr/bin/env python3
"""Build the production runtime secret (silicon-ring/production) for Silicon Ring 1.0.

  deploy/secrets.py            show what would be written: key names, sources, and the keys that
                               would be added, kept, changed or removed. Values are never printed.
  deploy/secrets.py --apply    write it with secretsmanager put-secret-value

The secret is rebuilt from scratch on every run, so every key the 1.0 server and media worker do
not read (IAM, Honeycomb, telemetry, organisations, test realms) disappears. Generated values
(LiveKit key and secret, media secret, encryption key) are created on the first 1.0 write and
reused afterwards; --rotate NAME regenerates one. Rotating RING_ENCRYPTION_KEY signs every
carbon and silicon out, because their Accounts refresh tokens are sealed with it.

Sources (all under secrets/, which is never committed):
  dev.env                                   OPENAI_API_KEY, DEEPGRAM_API_KEY, RING_APNS_KEY_ID/TEAM_ID/BUNDLE_ID
  AuthKey_<RING_APNS_KEY_ID>.p8             RING_APNS_KEY_BASE64
  firebase-service-account.json             RING_FCM_SERVICE_ACCOUNT_BASE64
  vapid/private.b64url, vapid/public.b64url RING_VAPID_PRIVATE_KEY, RING_VAPID_PUBLIC_KEY
  platform/ring-app.private.json            RING_ACCOUNTS_APP_SECRET (app_secret)
  platform/accounts-webhook.private.json    RING_ACCOUNTS_WEBHOOK_SECRET (the whsec_ value), when present
"""

import argparse
import base64
import hashlib
import json
import os
import pathlib
import subprocess
import sys
import tempfile

ROOT = pathlib.Path(__file__).resolve().parent.parent
SECRETS = ROOT / 'secrets'
SECRET_ID = 'silicon-ring/production'

FIXED = {
    'RUST_LOG': 'info',
    'RING_PUBLIC_URL': 'https://backend.ring.teamofsilicons.com',
    'RING_WEB_URL': 'https://ring.teamofsilicons.com',
    'RING_ALLOWED_ORIGINS': 'https://ring.teamofsilicons.com',
    'RING_ACCOUNTS_URL': 'https://accounts.teamofsilicons.com',
    'RING_ACCOUNTS_APP_ID': 'ring',
    'RING_TING_URL': 'https://backend.ting.teamofsilicons.com',
    'RING_LIVEKIT_URL': 'http://127.0.0.1:7880',
    'RING_LIVEKIT_PUBLIC_URL': 'wss://backend.ring.teamofsilicons.com',
    'RING_VAPID_SUBJECT': 'https://ring.teamofsilicons.com',
    'RING_S3_BUCKET': 'silicon-ring-assets-smdjuu0paizr',
    'RING_S3_PREFIX': 'production/',
    'AWS_REGION': 'us-west-1',
}


# This file is deploy/secrets.py, which shadows the standard library's `secrets` module when it
# runs as a script, so random values come from os.urandom directly.
def token_hex(size):
    return os.urandom(size).hex()


def token_urlsafe(size):
    return base64.urlsafe_b64encode(os.urandom(size)).rstrip(b'=').decode()


GENERATED = {
    'RING_LIVEKIT_API_KEY': lambda: 'ring' + token_hex(8),
    'RING_LIVEKIT_API_SECRET': lambda: token_urlsafe(48),
    'RING_MEDIA_SECRET': lambda: token_urlsafe(48),
    'RING_ENCRYPTION_KEY': lambda: token_hex(32),
}
# Present only once a secret has been written by this script; older secrets get fresh values.
MIGRATED_MARKER = 'RING_LIVEKIT_API_KEY'


def fail(message):
    raise SystemExit(f'secrets: {message}')


def read_env(path):
    values = {}
    for line in path.read_text().splitlines():
        line = line.strip()
        if line and not line.startswith('#') and '=' in line:
            key, value = line.split('=', 1)
            values[key.strip()] = value.strip().strip('"').strip("'")
    return values


def read_text(path):
    if not path.is_file():
        fail(f'missing {path.relative_to(ROOT)}')
    return path.read_text().strip()


def b64_file(path):
    if not path.is_file():
        fail(f'missing {path.relative_to(ROOT)}')
    return base64.b64encode(path.read_bytes()).decode()


def from_files():
    dev = read_env(SECRETS / 'dev.env')
    values, sources = {}, {}

    def put(key, value, source):
        if not value:
            fail(f'{key} is empty in {source}')
        values[key], sources[key] = value, source

    for key in ('OPENAI_API_KEY', 'DEEPGRAM_API_KEY'):
        put(key, dev.get(key), 'secrets/dev.env')
    put('RING_APNS_KEY_ID', dev.get('RING_APNS_KEY_ID', '4T8HU7N96S'), 'secrets/dev.env')
    put('RING_APNS_TEAM_ID', dev.get('RING_APNS_TEAM_ID', 'LTBSK59BJ2'), 'secrets/dev.env')
    put('RING_APNS_BUNDLE_ID', dev.get('RING_APNS_BUNDLE_ID', 'com.teamofsilicons.ring'), 'secrets/dev.env')
    apns = SECRETS / f"AuthKey_{values['RING_APNS_KEY_ID']}.p8"
    put('RING_APNS_KEY_BASE64', b64_file(apns), f'secrets/{apns.name}')
    put('RING_FCM_SERVICE_ACCOUNT_BASE64', b64_file(SECRETS / 'firebase-service-account.json'),
        'secrets/firebase-service-account.json')
    put('RING_VAPID_PRIVATE_KEY', read_text(SECRETS / 'vapid' / 'private.b64url'), 'secrets/vapid/private.b64url')
    put('RING_VAPID_PUBLIC_KEY', read_text(SECRETS / 'vapid' / 'public.b64url'), 'secrets/vapid/public.b64url')
    app = json.loads(read_text(SECRETS / 'platform' / 'ring-app.private.json'))
    put('RING_ACCOUNTS_APP_SECRET', app.get('app_secret'), 'secrets/platform/ring-app.private.json')
    webhook = SECRETS / 'platform' / 'accounts-webhook.private.json'
    if webhook.is_file():
        # Saved CLI output (`silicon-apps webhook ring set|rotate --json`); find the whsec_ wherever it sits.
        put('RING_ACCOUNTS_WEBHOOK_SECRET', find_webhook_secret(json.loads(webhook.read_text())),
            f'secrets/platform/{webhook.name}')
    return values, sources


def find_webhook_secret(node):
    if isinstance(node, str):
        return node if node.startswith('whsec_') else None
    children = node.values() if isinstance(node, dict) else node if isinstance(node, list) else []
    for child in children:
        found = find_webhook_secret(child)
        if found:
            return found
    return None


def aws(region, *args):
    result = subprocess.run(['aws', '--region', region, '--output', 'json', *args], text=True, capture_output=True)
    if result.returncode != 0:
        # The CLI's error names the operation and the secret, never its value.
        fail(result.stderr.strip().splitlines()[-1] if result.stderr.strip() else f'aws {args[0]} failed')
    return json.loads(result.stdout) if result.stdout.strip() else {}


def main():
    parser = argparse.ArgumentParser(description=__doc__, formatter_class=argparse.RawDescriptionHelpFormatter)
    parser.add_argument('--secret-id', default=SECRET_ID)
    parser.add_argument('--region', default='us-west-1')
    parser.add_argument('--rotate', action='append', default=[], choices=sorted(GENERATED), metavar='NAME',
                        help='regenerate one generated value (repeatable): ' + ', '.join(sorted(GENERATED)))
    parser.add_argument('--apply', action='store_true', help='write the secret (otherwise only show the plan)')
    args = parser.parse_args()

    current = json.loads(aws(args.region, 'secretsmanager', 'get-secret-value', '--secret-id', args.secret_id)['SecretString'])
    if not isinstance(current, dict):
        fail('the current secret is not a JSON object')
    migrated = MIGRATED_MARKER in current

    values, sources = from_files()
    for key, value in FIXED.items():
        values[key], sources[key] = value, 'fixed'
    for key, make in GENERATED.items():
        if migrated and key not in args.rotate and current.get(key):
            values[key], sources[key] = current[key], 'kept (generated earlier)'
        else:
            values[key], sources[key] = make(), 'generated now'
    webhook_key = 'RING_ACCOUNTS_WEBHOOK_SECRET'
    if webhook_key not in values and migrated and str(current.get(webhook_key, '')).startswith('whsec_'):
        values[webhook_key], sources[webhook_key] = current[webhook_key], 'kept (no local accounts-webhook.private.json)'

    added = sorted(set(values) - set(current))
    removed = sorted(set(current) - set(values))
    changed = sorted(k for k in set(values) & set(current) if values[k] != current[k])
    plan = {
        'secret_id': args.secret_id,
        'first_1_0_write': not migrated,
        'keys': {k: sources[k] for k in sorted(values)},
        'added': added,
        'changed': changed,
        'removed': removed,
    }
    print(json.dumps(plan, indent=2))
    if not migrated:
        print('note: the first 1.0 write uses a fresh RING_ENCRYPTION_KEY; old sealed data cannot be read with it.',
              file=sys.stderr)
    if not args.apply:
        print('dry run: nothing written (pass --apply to write)', file=sys.stderr)
        return 0
    if not (added or changed or removed):
        print('the secret is already up to date', file=sys.stderr)
        return 0

    body = json.dumps(values, sort_keys=True, separators=(',', ':'))
    token = hashlib.sha256(body.encode()).hexdigest()[:64]
    with tempfile.NamedTemporaryFile('w', suffix='.json', delete=False) as handle:
        os.chmod(handle.name, 0o600)
        handle.write(body)
    try:
        result = aws(args.region, 'secretsmanager', 'put-secret-value', '--secret-id', args.secret_id,
                     '--client-request-token', token, '--secret-string', f'file://{handle.name}')
    finally:
        os.unlink(handle.name)
    print(json.dumps({'written': True, 'version_id': result.get('VersionId'), 'keys': len(values)}))
    return 0


if __name__ == '__main__':
    sys.exit(main())
