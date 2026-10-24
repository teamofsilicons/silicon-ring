#!/usr/bin/env python3
"""Install and run the App Review demo silicon on the production host, over S3 and SSM.

  ship.py install                  upload agent.py, ring-demo.service and ring-demo.sh and install
                                   them on the host (the ring CLI comes from Ring's GitHub release)
  ship.py login [--home DIR] [--silicon ID]
                                   mint a Ring sign-in token from DIR's silicon-accounts sign-in as
                                   the demo silicon ID (default si:ring-demo), hand it to the host
                                   through S3, sign the host's ring CLI in, check it is ID, and
                                   start the agent, which then runs only as ID
  ship.py status | start | stop | logout
  ship.py remove [--purge]

The host, bucket and region come from the CloudFormation stack, as for deploy/deploy.py, whose SSM
runner this uses. Logs: deploy/deploy.py logs --unit ring-demo. Nothing here prints a secret, and
no token appears in an SSM command: the sign-in token travels through a private S3 object that is
deleted as soon as the host has read it.
"""

import argparse
import gzip
import hashlib
import io
import json
import pathlib
import re
import shlex
import subprocess
import sys
import tarfile
import tempfile
import uuid

HERE = pathlib.Path(__file__).resolve().parent
sys.path.insert(0, str(HERE.parent))
import deploy as ring_deploy  # noqa: E402  deploy/deploy.py: the stack lookup and SSM runner

FILES = ['agent.py', 'ring-demo.service', 'ring-demo.sh']
HOST_SCRIPT = '/opt/ring-demo/ring-demo.sh'
PREFIX = 'releases/demo-silicon'


def fail(message: str) -> None:
    raise SystemExit(f'ship: {message}')


def package() -> bytes:
    """A reproducible .tar.gz of FILES (same bytes for the same files)."""
    raw = io.BytesIO()
    with gzip.GzipFile(fileobj=raw, mode='wb', mtime=0) as gz:
        with tarfile.open(fileobj=gz, mode='w', format=tarfile.PAX_FORMAT) as tar:
            for name in FILES:
                data = (HERE / name).read_bytes()
                info = tarfile.TarInfo(name)
                info.size, info.mtime, info.uid, info.gid = len(data), 0, 0, 0
                info.mode = 0o755 if name.endswith('.sh') else 0o644
                tar.addfile(info, io.BytesIO(data))
    return raw.getvalue()


def cmd_install(args, aws) -> int:
    data = package()
    digest = hashlib.sha256(data).hexdigest()
    with tempfile.TemporaryDirectory() as tmp:
        path = pathlib.Path(tmp) / 'ring-demo.tar.gz'
        path.write_bytes(data)
        uri, digest = aws.staged_script(path, f'{PREFIX}/ring-demo-{digest[:12]}.tar.gz')
    region = shlex.quote(aws.region)
    script = '\n'.join([
        'set -euo pipefail',
        'work=$(mktemp -d /var/tmp/ring-demo.XXXXXXXX)',
        "trap 'rm -rf \"$work\"' EXIT",
        f'aws s3 cp {shlex.quote(uri)} "$work/ring-demo.tar.gz" --region {region} --only-show-errors',
        f'echo "{digest}  $work/ring-demo.tar.gz" | sha256sum --check --quiet -',
        f'tar -xzf "$work/ring-demo.tar.gz" -C "$work" {" ".join(FILES)}',
        'bash "$work/ring-demo.sh" install --from "$work"',
    ])
    return aws.ssm(script, 'Silicon Ring demo silicon: install', timeout=600)


def accounts(home: pathlib.Path, *args: str) -> str:
    """Runs silicon-accounts against DIR's sign-in. It never prompts: no stdin, no terminal."""
    try:
        done = subprocess.run(['silicon-accounts', '--home', str(home), *args], stdin=subprocess.DEVNULL,
                              capture_output=True, text=True, timeout=60, start_new_session=True)
    except FileNotFoundError:
        fail('silicon-accounts is not installed on this machine')
    except subprocess.TimeoutExpired:
        fail(f'silicon-accounts {args[0]} did not finish within 60 s')
    if done.returncode != 0:
        fail(f'silicon-accounts {" ".join(a for a in args if not a.startswith("-"))} failed: '
             f'{done.stderr.strip() or done.stdout.strip()}')
    return done.stdout


def cmd_login(args, aws) -> int:
    if not re.fullmatch(r'si:[a-z0-9_-]{3,30}', args.silicon):
        fail(f'--silicon {args.silicon!r} is not a silicon id (si: and 3 to 30 of a-z, 0-9, - and _)')
    home = args.home.expanduser()
    if not (home / '.accounts').is_dir():
        fail(f'{home} holds no silicon-accounts sign-in; sign the demo silicon in there first (README.md)')
    # Signed out, `login --app` would start a browser sign-in that a carbon could complete.
    try:
        who = json.loads(accounts(home, 'login', 'status', '--json') or '{}')
    except ValueError:
        fail('silicon-accounts login status --json printed something other than JSON')
    if not isinstance(who, dict):
        who = {}
    if who.get('authenticated') is not True or who.get('kind') != 'silicon' or who.get('id') != args.silicon:
        fail(f'{home} is not signed in as {args.silicon} (it reports {who.get("id") or "no one"}); '
             'sign the demo silicon in there first (README.md)')
    slt = accounts(home, 'login', '--app', 'ring', '-q').strip()
    if not slt or len(slt) > 4096 or any(c.isspace() for c in slt):
        fail('silicon-accounts did not print a Ring sign-in token')

    uri = f"s3://{aws.stack_info['bucket']}/{PREFIX}/login-{uuid.uuid4().hex}"
    region = shlex.quote(aws.region)
    try:
        upload = subprocess.run(['aws', '--region', aws.region, 's3', 'cp', '-', uri, '--sse', 'AES256',
                                 '--only-show-errors'], input=slt + '\n', text=True, capture_output=True)
        if upload.returncode != 0:
            fail(f'could not stage the sign-in token: {upload.stderr.strip()}')
        script = '\n'.join([
            'set -euo pipefail',
            f'uri={shlex.quote(uri)}',
            f"trap 'aws s3 rm \"$uri\" --region {region} --only-show-errors || true' EXIT",
            f'aws s3 cp "$uri" - --region {region} --only-show-errors | '
            f'{HOST_SCRIPT} login --silicon {shlex.quote(args.silicon)}',
        ])
        # The token is single use and lasts 2 minutes.
        return aws.ssm(script, 'Silicon Ring demo silicon: sign in', timeout=120)
    finally:
        subprocess.run(['aws', '--region', aws.region, 's3', 'rm', uri, '--only-show-errors'],
                       capture_output=True, text=True)


def host_command(command: str, comment: str):
    def run(args, aws) -> int:
        extra = ' --purge' if getattr(args, 'purge', False) else ''
        return aws.ssm(f'{HOST_SCRIPT} {command}{extra}', f'Silicon Ring demo silicon: {comment}', timeout=120)
    return run


def main() -> int:
    parser = argparse.ArgumentParser(description=__doc__, formatter_class=argparse.RawDescriptionHelpFormatter)
    parser.add_argument('--region', default='us-west-1')
    parser.add_argument('--stack', default='silicon-ring')
    sub = parser.add_subparsers(dest='command', required=True)
    sub.add_parser('install', help='install or update the demo silicon on the host')
    login = sub.add_parser('login', help="sign the host's ring CLI in as the demo silicon and start it")
    login.add_argument('--home', type=pathlib.Path, default=pathlib.Path('~/.ring-demo'),
                       help="directory whose silicon-accounts sign-in is the demo silicon's (default ~/.ring-demo)")
    login.add_argument('--silicon', default='si:ring-demo',
                       help='the demo silicon id (default si:ring-demo); the host agent then runs only as it')
    sub.add_parser('status', help='agent, sign-in, call backs due and recent log')
    sub.add_parser('start', help='start the agent (it stays signed in)')
    sub.add_parser('stop', help='stop the agent; calls to the demo silicon then ring out to voicemail')
    sub.add_parser('logout', help="stop the agent and revoke the host's Ring session")
    remove = sub.add_parser('remove', help='uninstall from the host')
    remove.add_argument('--purge', action='store_true', help='also sign out and delete the state')
    args = parser.parse_args()

    aws = ring_deploy.Aws(args.region, args.stack)
    handlers = {
        'install': cmd_install,
        'login': cmd_login,
        'status': host_command('status', 'status'),
        'start': host_command('start', 'start'),
        'stop': host_command('stop', 'stop'),
        'logout': host_command('logout', 'sign out'),
        'remove': host_command('remove', 'remove'),
    }
    return handlers[args.command](args, aws)


if __name__ == '__main__':
    sys.exit(main())
