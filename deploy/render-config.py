#!/usr/bin/env python3
"""Render the host configuration from the production secret (read on stdin).

Writes /etc/ring/server.env and /etc/ring/media.env (0600 root) for systemd's EnvironmentFile=,
and /etc/livekit/livekit.yaml (0640 root:livekit) from deploy/livekit.yaml.tmpl. Prints key names
only, never values. Runs on the host under Python 3.9 (Amazon Linux 2023), standard library only.
"""

import argparse
import grp
import json
import os
import pathlib
import re
import sys
import tempfile

# Settings each process reads (crates/ring-server/src/config.rs, crates/ring-media/src/config.rs).
SERVER_KEYS = {
    'RUST_LOG', 'RING_BIND', 'RING_DATA_DIR', 'RING_PUBLIC_URL', 'RING_WEB_URL', 'RING_ALLOWED_ORIGINS',
    'RING_ENCRYPTION_KEY', 'RING_ACCOUNTS_URL', 'RING_ACCOUNTS_APP_ID', 'RING_ACCOUNTS_APP_SECRET',
    'RING_ACCOUNTS_WEBHOOK_SECRET', 'RING_TING_URL', 'RING_LIVEKIT_URL', 'RING_LIVEKIT_PUBLIC_URL',
    'RING_LIVEKIT_API_KEY', 'RING_LIVEKIT_API_SECRET', 'RING_MEDIA_SECRET', 'RING_APNS_KEY_ID',
    'RING_APNS_TEAM_ID', 'RING_APNS_BUNDLE_ID', 'RING_APNS_KEY_BASE64', 'RING_FCM_SERVICE_ACCOUNT_BASE64',
    'RING_VAPID_PRIVATE_KEY', 'RING_VAPID_PUBLIC_KEY', 'RING_VAPID_SUBJECT', 'RING_S3_BUCKET',
    'RING_S3_PREFIX', 'AWS_REGION',
}
MEDIA_KEYS = {
    'RUST_LOG', 'RING_MEDIA_SERVER_URL', 'RING_MEDIA_SECRET', 'RING_LIVEKIT_URL', 'RING_MEDIA_DATA_DIR',
    'OPENAI_API_KEY', 'DEEPGRAM_API_KEY', 'RING_S3_BUCKET', 'AWS_REGION',
}
REQUIRED = {
    'RING_PUBLIC_URL', 'RING_WEB_URL', 'RING_ENCRYPTION_KEY', 'RING_ACCOUNTS_APP_SECRET',
    'RING_LIVEKIT_PUBLIC_URL', 'RING_LIVEKIT_API_KEY', 'RING_LIVEKIT_API_SECRET', 'RING_MEDIA_SECRET',
    'OPENAI_API_KEY', 'DEEPGRAM_API_KEY',
}
# The host layout decides these; the units' ReadWritePaths and Caddy's upstreams depend on them.
FIXED = {
    'RING_BIND': '127.0.0.1:8765',
    'RING_DATA_DIR': '/var/lib/ring',
    'RING_MEDIA_DATA_DIR': '/var/lib/ring/media',
    'RING_MEDIA_SERVER_URL': 'ws://127.0.0.1:8765/internal/media',
}
DEFAULTS = {
    'RUST_LOG': 'info',
    'RING_LIVEKIT_URL': 'http://127.0.0.1:7880',
}
# Retired with Silicon Ring 1.0: refuse to deploy while the secret still carries any of them.
RETIRED = re.compile(r'^(RING_IAM_|RING_TELEMETRY|RING_TEST_|RING_HONEYCOMB|HONEYCOMB_|SPACE_STATION|RING_OWNER_ORG$|'
                     r'RING_DEFAULT_ORG$|RING_TING_TEST_|RING_RELEASE_INDEX)')
LIVEKIT_TOKEN = re.compile(r'^[A-Za-z0-9_-]+$')


def env_line(key: str, value: str) -> str:
    # systemd parses EnvironmentFile values with shell double-quote rules and never expands them.
    escaped = value.replace('\\', '\\\\').replace('"', '\\"').replace('$', '\\$').replace('`', '\\`')
    return f'{key}="{escaped}"\n'


def write_atomic(path: pathlib.Path, text: str, mode: int, group: str = 'root') -> None:
    path.parent.mkdir(parents=True, exist_ok=True)
    fd, tmp = tempfile.mkstemp(prefix=f'.{path.name}.', dir=str(path.parent))
    try:
        with os.fdopen(fd, 'w') as handle:
            handle.write(text)
        os.chmod(tmp, mode)
        os.chown(tmp, 0, grp.getgrnam(group).gr_gid)
        os.replace(tmp, path)
    except BaseException:
        os.unlink(tmp)
        raise


def main() -> int:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--template', type=pathlib.Path, required=True)
    parser.add_argument('--bucket', required=True)
    parser.add_argument('--region', required=True)
    parser.add_argument('--etc', type=pathlib.Path, default=pathlib.Path('/etc'))
    parser.add_argument('--livekit-group', default='livekit')
    args = parser.parse_args()

    try:
        secret = json.loads(sys.stdin.read())
    except ValueError:
        print('the runtime secret is not a JSON object', file=sys.stderr)
        return 1
    if not isinstance(secret, dict):
        print('the runtime secret is not a JSON object', file=sys.stderr)
        return 1

    problems = []
    retired = sorted(k for k in secret if RETIRED.match(k))
    if retired:
        problems.append('retired keys still in the secret (run deploy/secrets.py): ' + ', '.join(retired))
    for key, value in secret.items():
        if not re.fullmatch(r'[A-Z][A-Z0-9_]*', key):
            problems.append(f'invalid key name {key!r}')
        elif not isinstance(value, str) or any(c in value for c in '\0\r\n'):
            problems.append(f'{key} must be a single-line string')
    values = {**DEFAULTS, 'RING_S3_BUCKET': args.bucket, 'AWS_REGION': args.region}
    values.update({k: v for k, v in secret.items() if isinstance(v, str) and v.strip()})
    overridden = sorted(k for k in FIXED if k in secret and secret[k] != FIXED[k])
    values.update(FIXED)
    missing = sorted(k for k in REQUIRED if not values.get(k))
    if missing:
        problems.append('missing required keys: ' + ', '.join(missing))
    api_key, api_secret = values.get('RING_LIVEKIT_API_KEY', ''), values.get('RING_LIVEKIT_API_SECRET', '')
    if api_key and not LIVEKIT_TOKEN.match(api_key):
        problems.append('RING_LIVEKIT_API_KEY must use only letters, digits, - and _')
    if api_secret and (not LIVEKIT_TOKEN.match(api_secret) or len(api_secret) < 32):
        problems.append('RING_LIVEKIT_API_SECRET must be at least 32 letters, digits, - or _')
    if problems:
        for problem in problems:
            print(problem, file=sys.stderr)
        return 1

    unknown = sorted(k for k in secret if k not in SERVER_KEYS | MEDIA_KEYS)
    if unknown:
        print('ignoring keys no process reads: ' + ', '.join(unknown))
    if overridden:
        print('host layout overrides: ' + ', '.join(overridden))

    media = dict(values)
    media['RING_LIVEKIT_URL'] = re.sub(r'^http', 'ws', values['RING_LIVEKIT_URL'])
    files = {
        'ring/server.env': ''.join(env_line(k, values[k]) for k in sorted(SERVER_KEYS) if values.get(k)),
        'ring/media.env': ''.join(env_line(k, media[k]) for k in sorted(MEDIA_KEYS) if media.get(k)),
    }
    for name, text in files.items():
        write_atomic(args.etc / name, '# Rendered from the production secret by deploy/render-config.py.\n' + text, 0o600)
        print(f'/etc/{name}: ' + ', '.join(line.split('=', 1)[0] for line in text.splitlines()))

    livekit = args.template.read_text()
    livekit = livekit.replace('{{LIVEKIT_API_KEY}}', api_key).replace('{{LIVEKIT_API_SECRET}}', api_secret)
    if '{{' in livekit:
        print('livekit.yaml.tmpl has a placeholder this renderer does not know', file=sys.stderr)
        return 1
    write_atomic(args.etc / 'livekit/livekit.yaml', livekit, 0o640, args.livekit_group)
    print('/etc/livekit/livekit.yaml: rendered')
    return 0


if __name__ == '__main__':
    sys.exit(main())
