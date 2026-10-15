#!/usr/bin/env python3
"""Package and ship Silicon Ring to the production host over S3 and SSM.

  deploy.py package                    build dist/release/<id>.tar.gz from dist/server, web/out, deploy/
  deploy.py release [--package P]      package (or reuse P), upload it, install it over SSM, print the result
  deploy.py rollback [--to ID] [--restore-db]
  deploy.py status | logs [--unit U] [--lines N] [--since S]
  deploy.py backup [--label L]         copy the host's SQLite databases to s3://<bucket>/backups/
  deploy.py stack [--execute]          preview (and optionally apply) deploy/stack.yaml as a change set

Instance, bucket and secret come from the CloudFormation stack (default silicon-ring, us-west-1).
Nothing here prints a secret: the host-side scripts report key names and paths only.
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
import time

ROOT = pathlib.Path(__file__).resolve().parent.parent
DEPLOY = ROOT / 'deploy'
DIST = ROOT / 'dist'
DEPLOY_FILES = ['Caddyfile', 'livekit.yaml.tmpl', 'install-release.sh', 'render-config.py', 'backup.py']
UNIT_FILES = ['ring-server.service', 'ring-media.service', 'livekit.service', 'caddy.service',
              'ring-backup.service', 'ring-backup.timer']
AMI_PARAMETER = '/aws/service/ami-amazon-linux-latest/al2023-ami-kernel-default-arm64'
PENDING = {'Pending', 'InProgress', 'Delayed'}


def fail(message: str) -> None:
    raise SystemExit(f'deploy: {message}')


def run(*args: str, capture: bool = True) -> str:
    result = subprocess.run(args, check=False, text=True, capture_output=capture)
    if result.returncode != 0:
        detail = (result.stderr or '').strip() if capture else ''
        fail(f'{args[0]} {args[1] if len(args) > 1 else ""} failed{": " + detail if detail else ""}')
    return result.stdout if capture else ''


class Aws:
    def __init__(self, region: str, stack: str):
        self.region, self.stack = region, stack
        self._stack = None

    def __call__(self, *args: str) -> str:
        return run('aws', '--region', self.region, '--output', 'json', *args)

    def json(self, *args: str):
        out = self(*args)
        return json.loads(out) if out.strip() else {}

    @property
    def stack_info(self) -> dict:
        if self._stack is None:
            stack = self.json('cloudformation', 'describe-stacks', '--stack-name', self.stack)['Stacks'][0]
            outputs = {o['OutputKey']: o['OutputValue'] for o in stack.get('Outputs', [])}
            params = {p['ParameterKey']: p.get('ParameterValue') for p in stack.get('Parameters', [])}
            self._stack = {
                'instance': outputs['InstanceId'],
                'bucket': outputs['AssetsBucket'],
                'secret': outputs.get('SecretsArn') or params['SecretsArn'],
                'status': stack['StackStatus'],
            }
        return self._stack

    def ssm(self, script: str, comment: str, timeout: int = 1800) -> int:
        info = self.stack_info
        with tempfile.NamedTemporaryFile('w', suffix='.json') as request:
            json.dump({
                'DocumentName': 'AWS-RunShellScript',
                'InstanceIds': [info['instance']],
                'Comment': comment[:100],
                'TimeoutSeconds': 60,
                'Parameters': {'commands': [script], 'executionTimeout': [str(timeout)]},
            }, request)
            request.flush()
            command = self.json('ssm', 'send-command', '--cli-input-json', f'file://{request.name}')['Command']
        command_id = command['CommandId']
        print(json.dumps({'instance': info['instance'], 'command_id': command_id, 'comment': comment}), flush=True)
        deadline = time.monotonic() + timeout + 120
        while time.monotonic() < deadline:
            time.sleep(3)
            try:
                result = self.json('ssm', 'get-command-invocation', '--command-id', command_id,
                                   '--instance-id', info['instance'])
            except SystemExit:
                continue  # the invocation is not visible for a moment after send-command
            if result['Status'] in PENDING:
                continue
            sys.stdout.write(result.get('StandardOutputContent', ''))
            sys.stderr.write(result.get('StandardErrorContent', ''))
            print(f"\nSSM {result['Status']} (exit {result.get('ResponseCode')})", flush=True)
            return 0 if result['Status'] == 'Success' else 1
        fail(f'command {command_id} is still running; inspect it before retrying')
        return 1

    def staged_script(self, source: pathlib.Path, key: str) -> tuple:
        """Upload a host-side script under releases/ and return (s3 uri, sha256)."""
        digest = hashlib.sha256(source.read_bytes()).hexdigest()
        uri = f"s3://{self.stack_info['bucket']}/{key}"
        self('s3', 'cp', str(source), uri, '--sse', 'AES256', '--only-show-errors')
        return uri, digest


def git(*args: str) -> str:
    try:
        return subprocess.run(['git', '-C', str(ROOT), *args], check=True, text=True, capture_output=True).stdout.strip()
    except (OSError, subprocess.CalledProcessError):
        return ''


def workspace_version() -> str:
    manifest = (ROOT / 'Cargo.toml').read_text()
    match = re.search(r'^\[workspace\.package\][^\[]*?^version\s*=\s*"([^"]+)"', manifest, re.M | re.S)
    if not match:
        fail('no [workspace.package] version in Cargo.toml')
    return match.group(1)


def check_aarch64_elf(path: pathlib.Path) -> None:
    header = path.read_bytes()[:20]
    if header[:4] != b'\x7fELF' or header[4] != 2 or header[18:20] != b'\xb7\x00':
        fail(f'{path} is not a 64-bit aarch64 Linux executable (build it with scripts/build-linux-server.sh)')


def release_entries(server: pathlib.Path, web: pathlib.Path) -> list:
    """(archive path, source path, mode) for every file in a release, sorted."""
    entries = []
    for name in ('ring-server', 'ring-media'):
        source = server / name
        if not source.is_file():
            fail(f'missing {source}; run scripts/build-linux-server.sh')
        check_aarch64_elf(source)
        entries.append((f'bin/{name}', source, 0o755))
    if not (web / 'index.html').is_file():
        fail(f'missing {web}/index.html; build the web app (cd web && npm run build)')
    for source in sorted(web.rglob('*')):
        if source.is_file() and source.name != '.DS_Store':
            entries.append((f'web/{source.relative_to(web).as_posix()}', source, 0o644))
    for name in DEPLOY_FILES:
        entries.append((f'deploy/{name}', DEPLOY / name, 0o755 if name.endswith(('.sh', '.py')) else 0o644))
    for name in UNIT_FILES:
        entries.append((f'deploy/systemd/{name}', DEPLOY / 'systemd' / name, 0o644))
    return sorted(entries)


def package(server: pathlib.Path, web: pathlib.Path, out: pathlib.Path) -> dict:
    entries = release_entries(server, web)
    content = hashlib.sha256()
    for arcname, source, mode in entries:
        content.update(f'{arcname}\0{mode:o}\0{hashlib.sha256(source.read_bytes()).hexdigest()}\n'.encode())
    version = workspace_version()
    revision = git('rev-parse', 'HEAD') or 'unknown'
    dirty = bool(git('status', '--porcelain', '--', 'crates', 'web', 'deploy', 'Cargo.toml', 'Cargo.lock'))
    release_id = f'{version}-{revision[:12]}-{content.hexdigest()[:8]}'
    mtime = int(git('log', '-1', '--format=%ct') or 0)
    server_build = server / 'build.json'
    web_version = json.loads((ROOT / 'web' / 'package.json').read_text()).get('version')
    manifest = {
        'id': release_id,
        'version': version,
        'web_version': web_version,
        'source_revision': revision,
        'source_dirty': dirty,
        'content_sha256': content.hexdigest(),
        'server_build': json.loads(server_build.read_text()) if server_build.is_file() else None,
        'files': len(entries),
    }

    def add(tar: tarfile.TarFile, name: str, data: bytes, mode: int) -> None:
        info = tarfile.TarInfo(name)
        info.size, info.mode, info.mtime = len(data), mode, mtime
        info.uid = info.gid = 0
        info.uname = info.gname = 'root'
        tar.addfile(info, io.BytesIO(data))

    out.mkdir(parents=True, exist_ok=True)
    archive = out / f'{release_id}.tar.gz'
    with open(archive, 'wb') as raw, gzip.GzipFile(fileobj=raw, mode='wb', mtime=0, filename='') as gz, \
            tarfile.open(fileobj=gz, mode='w', format=tarfile.PAX_FORMAT) as tar:
        directories = sorted({str(pathlib.PurePosixPath(a).parent) for a, _, _ in entries} - {'.'})
        expanded = set()
        for directory in directories:
            parts = pathlib.PurePosixPath(directory).parts
            for i in range(1, len(parts) + 1):
                expanded.add('/'.join(parts[:i]))
        for directory in sorted(expanded):
            info = tarfile.TarInfo(directory)
            info.type, info.mode, info.mtime = tarfile.DIRTYPE, 0o755, mtime
            info.uname = info.gname = 'root'
            tar.addfile(info)
        add(tar, 'RELEASE.json', (json.dumps(manifest, indent=2) + '\n').encode(), 0o644)
        for arcname, source, mode in entries:
            add(tar, arcname, source.read_bytes(), mode)
    digest = hashlib.sha256(archive.read_bytes()).hexdigest()
    (out / f'{release_id}.json').write_text(json.dumps({**manifest, 'archive': str(archive), 'sha256': digest}, indent=2) + '\n')
    if dirty:
        print('warning: the working tree has uncommitted changes in shipped paths', file=sys.stderr)
    return {'id': release_id, 'archive': str(archive), 'sha256': digest, 'size': archive.stat().st_size}


def load_package(path: pathlib.Path) -> dict:
    meta = path.with_suffix('').with_suffix('.json')
    if not meta.is_file():
        fail(f'{meta} not found next to the package')
    info = json.loads(meta.read_text())
    digest = hashlib.sha256(path.read_bytes()).hexdigest()
    if digest != info['sha256']:
        fail(f'{path} does not match its manifest')
    return {'id': info['id'], 'archive': str(path), 'sha256': digest, 'size': path.stat().st_size}


def cmd_package(args) -> int:
    print(json.dumps(package(args.server, args.web, args.out), indent=2))
    return 0


def cmd_release(args, aws: Aws) -> int:
    built = load_package(args.package) if args.package else package(args.server, args.web, args.out)
    info = aws.stack_info
    bucket, release_id = info['bucket'], built['id']
    archive_key = f'releases/{release_id}.tar.gz'
    archive_uri = f's3://{bucket}/{archive_key}'
    print(json.dumps({'release': release_id, 'sha256': built['sha256'], 'archive': archive_uri,
                      'instance': info['instance']}), flush=True)
    if args.dry_run:
        return 0
    try:
        head = aws.json('s3api', 'head-object', '--bucket', bucket, '--key', archive_key)
        uploaded = head.get('Metadata', {}).get('sha256') == built['sha256']
    except SystemExit:
        uploaded = False
    if not uploaded:
        aws('s3', 'cp', built['archive'], archive_uri, '--sse', 'AES256', '--only-show-errors',
            '--metadata', f"sha256={built['sha256']}")
    installer_uri, installer_sha = aws.staged_script(DEPLOY / 'install-release.sh', f'releases/{release_id}.install-release.sh')

    install = ['install', '--id', release_id, '--archive', archive_uri, '--sha256', built['sha256'],
               '--secret-id', info['secret'], '--bucket', bucket, '--region', aws.region]
    if args.restore_backup:
        install += ['--restore-backup', args.restore_backup]
    script = '\n'.join([
        'set -euo pipefail',
        'umask 077',
        'work=$(mktemp -d)',
        "trap 'rm -rf \"$work\"' EXIT",
        f'aws s3 cp {shlex.quote(installer_uri)} "$work/install-release.sh" --region {shlex.quote(aws.region)} --only-show-errors',
        f'echo {shlex.quote(installer_sha)}"  $work/install-release.sh" | sha256sum --check --quiet -',
        'bash "$work/install-release.sh" ' + ' '.join(shlex.quote(a) for a in install),
    ])
    return aws.ssm(script, f'Silicon Ring release {release_id}')


def host_installer(*args: str) -> str:
    return 'bash /opt/ring/current/deploy/install-release.sh ' + ' '.join(shlex.quote(a) for a in args)


def cmd_rollback(args, aws: Aws) -> int:
    info = aws.stack_info
    command = ['rollback', '--secret-id', info['secret'], '--bucket', info['bucket'], '--region', aws.region]
    if args.to:
        command += ['--to', args.to]
    if args.restore_db:
        command.append('--restore-db')
    return aws.ssm(host_installer(*command), 'Silicon Ring rollback')


def cmd_status(args, aws: Aws) -> int:
    return aws.ssm(host_installer('status'), 'Silicon Ring status', timeout=120)


def cmd_logs(args, aws: Aws) -> int:
    if not re.fullmatch(r'[a-z0-9.@-]+', args.unit):
        fail('invalid unit name')
    script = f'journalctl --no-pager --output short-iso -u {shlex.quote(args.unit)} -n {int(args.lines)}'
    if args.since:
        script += f' --since {shlex.quote(args.since)}'
    return aws.ssm(script, f'Silicon Ring logs {args.unit}', timeout=120)


def cmd_backup(args, aws: Aws) -> int:
    info = aws.stack_info
    source = DEPLOY / 'backup.py'
    digest = hashlib.sha256(source.read_bytes()).hexdigest()
    uri, digest = aws.staged_script(source, f'releases/tools/backup-{digest[:12]}.py')
    if not re.fullmatch(r'[a-z0-9-]{1,40}', args.label):
        fail('--label must be lowercase letters, digits and dashes')
    script = '\n'.join([
        'set -euo pipefail',
        'umask 077',
        'work=$(mktemp -d)',
        "trap 'rm -rf \"$work\"' EXIT",
        f'aws s3 cp {shlex.quote(uri)} "$work/backup.py" --region {shlex.quote(aws.region)} --only-show-errors',
        f'echo {shlex.quote(digest)}"  $work/backup.py" | sha256sum --check --quiet -',
        f'python3 -I "$work/backup.py" --bucket {shlex.quote(info["bucket"])} --region {shlex.quote(aws.region)} '
        f'--label {shlex.quote(args.label)}',
    ])
    return aws.ssm(script, f'Silicon Ring backup {args.label}', timeout=900)


def pinned_ami(args, aws: Aws) -> str:
    """The image to keep the host on, or '' to launch a new host from the latest AL2023 arm64 image."""
    if args.new_ami:
        return ''
    reservations = aws.json('ec2', 'describe-instances', '--instance-ids', aws.stack_info['instance'])['Reservations']
    instance = reservations[0]['Instances'][0]
    if instance.get('Architecture') == 'arm64':
        return instance['ImageId']
    # The pre-1.0 host is x86_64; moving to Graviton replaces it once.
    print(f"note: {instance['InstanceId']} is {instance.get('Architecture')}, so this update replaces it with a "
          't4g host; take a backup first (deploy.py backup)', file=sys.stderr)
    return ''


def cmd_stack(args, aws: Aws) -> int:
    name = f"ring-{time.strftime('%Y%m%d-%H%M%S')}"
    parameters = [
        {'ParameterKey': 'VpcId', 'UsePreviousValue': True},
        {'ParameterKey': 'SubnetId', 'UsePreviousValue': True},
        {'ParameterKey': 'SecretsArn', 'UsePreviousValue': True},
        {'ParameterKey': 'InstanceType', 'ParameterValue': args.instance_type},
        {'ParameterKey': 'LatestAmi', 'ParameterValue': AMI_PARAMETER},
        {'ParameterKey': 'PinnedAmi', 'ParameterValue': pinned_ami(args, aws)},
    ]
    aws('cloudformation', 'create-change-set', '--stack-name', aws.stack, '--change-set-name', name,
        '--template-body', f'file://{DEPLOY / "stack.yaml"}', '--capabilities', 'CAPABILITY_IAM',
        '--parameters', json.dumps(parameters))
    try:
        aws('cloudformation', 'wait', 'change-set-create-complete', '--stack-name', aws.stack, '--change-set-name', name)
    except SystemExit:
        pass  # a change set with no changes ends FAILED; describe it below either way
    described = aws.json('cloudformation', 'describe-change-set', '--stack-name', aws.stack, '--change-set-name', name)
    print(f"change set {name}: {described.get('Status')} {described.get('StatusReason', '')}".rstrip())
    for change in described.get('Changes', []):
        rc = change['ResourceChange']
        print(f"  {rc['Action']:<8} {rc['LogicalResourceId']:<16} {rc['ResourceType']:<28} replacement={rc.get('Replacement', '-')}")
    if not args.execute or described.get('Status') != 'CREATE_COMPLETE':
        print('not executed; rerun with --execute to apply (run deploy.py backup first if Instance is replaced)')
        return 0
    aws('cloudformation', 'execute-change-set', '--stack-name', aws.stack, '--change-set-name', name)
    aws('cloudformation', 'wait', 'stack-update-complete', '--stack-name', aws.stack)
    aws._stack = None
    print(json.dumps(aws.stack_info, indent=2))
    return 0


def main() -> int:
    parser = argparse.ArgumentParser(description=__doc__, formatter_class=argparse.RawDescriptionHelpFormatter)
    parser.add_argument('--region', default='us-west-1')
    parser.add_argument('--stack', default='silicon-ring')
    sub = parser.add_subparsers(dest='command', required=True)

    def packaging(p):
        p.add_argument('--server', type=pathlib.Path, default=DIST / 'server', help='directory with ring-server and ring-media')
        p.add_argument('--web', type=pathlib.Path, default=ROOT / 'web' / 'out', help='the static web export')
        p.add_argument('--out', type=pathlib.Path, default=DIST / 'release')

    packaging(sub.add_parser('package', help='build the release archive locally'))
    release = sub.add_parser('release', help='package, upload and install a release')
    packaging(release)
    release.add_argument('--package', type=pathlib.Path, help='ship an archive made by `deploy.py package`')
    release.add_argument('--restore-backup', help='s3:// URI of a backup to restore on a host with no database')
    release.add_argument('--dry-run', action='store_true', help='package and resolve the target, but upload nothing')
    rollback = sub.add_parser('rollback', help='switch back to the previous (or a given) release')
    rollback.add_argument('--to')
    rollback.add_argument('--restore-db', action='store_true', help='also restore the database snapshot taken before the current release')
    sub.add_parser('status', help='release, services and health on the host')
    logs = sub.add_parser('logs', help='journal lines of one unit')
    logs.add_argument('--unit', default='ring-server')
    logs.add_argument('--lines', type=int, default=200)
    logs.add_argument('--since')
    backup = sub.add_parser('backup', help='back up the host databases to S3')
    backup.add_argument('--label', default='manual')
    stack = sub.add_parser('stack', help='preview or apply deploy/stack.yaml')
    stack.add_argument('--instance-type', default='t4g.medium')
    stack.add_argument('--new-ami', action='store_true',
                       help='move to the latest AL2023 arm64 image (replaces the instance; back up first)')
    stack.add_argument('--execute', action='store_true')

    args = parser.parse_args()
    if args.command == 'package':
        return cmd_package(args)
    aws = Aws(args.region, args.stack)
    handlers = {'release': cmd_release, 'rollback': cmd_rollback, 'status': cmd_status, 'logs': cmd_logs,
                'backup': cmd_backup, 'stack': cmd_stack}
    return handlers[args.command](args, aws)


if __name__ == '__main__':
    sys.exit(main())
