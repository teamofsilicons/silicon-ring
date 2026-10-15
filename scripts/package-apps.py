#!/usr/bin/env python3
"""Build the Silicon Apps packages for the ring CLI from scripts/build-cli.sh output.

One package per live Silicon Apps target (Linux only today), each a directory with apps.yaml,
bin/ring and LICENSE, checked with `silicon-apps validate` and packed with `silicon-apps pack`
(both local) into dist/apps/ring-apps-<version>-<target>.tar.gz, plus dist/apps/SHA256SUMS and
dist/apps/packages.json for scripts/publish-apps.py.

When an arm64 docker host is available, the aarch64 package is also run the way the Apps
validator runs it: no network, read-only root, uid 65534, HOME=SILICON_HOME=/tmp.
"""

import argparse
import hashlib
import json
import os
import pathlib
import re
import shutil
import subprocess
import sys
import tarfile

ROOT = pathlib.Path(__file__).resolve().parent.parent
TARGETS = {
    'linux-x86_64': 'x86_64-unknown-linux-musl',
    'linux-aarch64': 'aarch64-unknown-linux-musl',
}
ELF_MACHINE = {'linux-x86_64': b'\x3e\x00', 'linux-aarch64': b'\xb7\x00'}
VALIDATOR_IMAGE = 'python:3.14-slim-trixie'


def fail(message):
    raise SystemExit(f'package-apps: {message}')


def workspace_version():
    manifest = (ROOT / 'Cargo.toml').read_text()
    match = re.search(r'^\[workspace\.package\][^\[]*?^version\s*=\s*"([^"]+)"', manifest, re.M | re.S)
    if not match:
        fail('no [workspace.package] version in Cargo.toml')
    return match.group(1)


def checksums(path):
    sums = {}
    for line in path.read_text().splitlines():
        digest, _, name = line.partition('  ')
        sums[name.strip().lstrip('*')] = digest.strip()
    return sums


def extract_binary(archive, destination):
    with tarfile.open(archive, 'r:gz') as tar:
        member = next((m for m in tar.getmembers() if m.isfile() and pathlib.PurePosixPath(m.name).name == 'ring'), None)
        if member is None:
            fail(f'{archive.name} has no ring binary')
        destination.write_bytes(tar.extractfile(member).read())
    destination.chmod(0o755)


def silicon_apps(*args):
    env = {**os.environ, 'SILICON_APPS_NO_DAEMON': '1'}
    result = subprocess.run(['silicon-apps', *args, '--json'], text=True, capture_output=True, env=env)
    if result.returncode != 0:
        fail(f'silicon-apps {args[0]} failed: {(result.stdout + result.stderr).strip()}')
    return json.loads(result.stdout) if result.stdout.strip() else {}


def use_anonymous_docker_config():
    """Pull public images even when the configured credential helper is not installed.

    A leftover Docker Desktop credsStore (colima in use) makes every pull fail, so point the client
    at an empty config on the same daemon, as scripts/build-linux-server.sh does.
    """
    config = pathlib.Path(os.environ.get('DOCKER_CONFIG', pathlib.Path.home() / '.docker')) / 'config.json'
    try:
        store = json.loads(config.read_text()).get('credsStore', '')
    except (OSError, ValueError):
        return
    if not store or shutil.which(f'docker-credential-{store}'):
        return
    if not os.environ.get('DOCKER_HOST'):
        host = subprocess.run(['docker', 'context', 'inspect', '--format', '{{.Endpoints.docker.Host}}'],
                              text=True, capture_output=True)
        if host.returncode == 0 and host.stdout.strip():
            os.environ['DOCKER_HOST'] = host.stdout.strip()
    empty = ROOT / 'target' / 'infra' / 'docker-config'
    empty.mkdir(parents=True, exist_ok=True)
    if not (empty / 'config.json').exists():
        (empty / 'config.json').write_text('{}\n')
    os.environ['DOCKER_CONFIG'] = str(empty)


def docker_arch():
    use_anonymous_docker_config()
    try:
        out = subprocess.run(['docker', 'info', '--format', '{{.Architecture}}'], text=True, capture_output=True, timeout=20)
    except (OSError, subprocess.TimeoutExpired):
        return None
    return out.stdout.strip() if out.returncode == 0 else None


def sandbox_check(stage):
    """Run the validator's three commands the way its Linux runner does."""
    pull = subprocess.run(['docker', 'pull', '--quiet', '--platform', 'linux/arm64', VALIDATOR_IMAGE],
                          text=True, capture_output=True, timeout=900)
    if pull.returncode != 0:
        fail(f'cannot pull {VALIDATOR_IMAGE}: {pull.stderr.strip()[:400]}')
    base = ['docker', 'run', '--rm', '--platform', 'linux/arm64', '--network=none', '--read-only', '--cap-drop=ALL',
            '--security-opt=no-new-privileges', '--pids-limit=32', '--memory=256m', '--cpus=1', '--user=65534:65534',
            '--tmpfs=/tmp:rw,noexec,nosuid,size=16m', '-e', 'HOME=/tmp', '-e', 'SILICON_HOME=/tmp',
            '-v', f'{stage}:/package:ro', '-w', '/package', '--entrypoint', '/package/bin/ring', VALIDATOR_IMAGE]
    results = {}
    for command in (['--help'], ['accounts', '--json'], ['login', 'status', '--json']):
        run = subprocess.run(base + command, text=True, capture_output=True, timeout=120)
        name = ' '.join(command)
        if run.returncode != 0:
            fail(f'`ring {name}` exited {run.returncode} in the validator sandbox: {run.stderr.strip()[:400]}')
        if command[-1] == '--json':
            data = json.loads(run.stdout)
            if command[0] == 'accounts' and data.get('app_id') != 'ring':
                fail('`ring accounts --json` does not report app_id "ring"')
            if command[0] == 'login' and data.get('authenticated') is not False:
                fail('`ring login status --json` is not authenticated:false when signed out')
        elif not (run.stdout + run.stderr).strip():
            fail('`ring --help` printed nothing')
        results[name] = 'passed'
    return results


def main():
    parser = argparse.ArgumentParser(description=__doc__, formatter_class=argparse.RawDescriptionHelpFormatter)
    parser.add_argument('--cli', type=pathlib.Path, default=ROOT / 'dist' / 'cli', help='scripts/build-cli.sh output')
    parser.add_argument('--out', type=pathlib.Path, default=ROOT / 'dist' / 'apps')
    parser.add_argument('--target', action='append', choices=sorted(TARGETS), help='only these Apps targets')
    parser.add_argument('--no-sandbox', action='store_true', help='skip the docker validator run')
    args = parser.parse_args()

    if not shutil.which('silicon-apps'):
        fail('silicon-apps is not installed')
    version = workspace_version()
    sums = checksums(args.cli / 'SHA256SUMS')
    stage_root = args.out / 'stage'
    shutil.rmtree(stage_root, ignore_errors=True)
    args.out.mkdir(parents=True, exist_ok=True)
    sandbox = not args.no_sandbox and docker_arch() in ('aarch64', 'arm64')

    packages = []
    for apps_target in args.target or sorted(TARGETS):
        rust_target = TARGETS[apps_target]
        archive = args.cli / f'ring-{rust_target}.tar.gz'
        if not archive.is_file():
            fail(f'missing {archive}; run scripts/build-cli.sh {rust_target}')
        if hashlib.sha256(archive.read_bytes()).hexdigest() != sums.get(archive.name):
            fail(f'{archive.name} does not match {args.cli}/SHA256SUMS')

        stage = stage_root / f'ring-{apps_target}'
        (stage / 'bin').mkdir(parents=True)
        binary = stage / 'bin' / 'ring'
        extract_binary(archive, binary)
        header = binary.read_bytes()[:20]
        if header[:4] != b'\x7fELF' or header[18:20] != ELF_MACHINE[apps_target]:
            fail(f'the binary in {archive.name} is not a {apps_target} ELF')
        shutil.copyfile(ROOT / 'LICENSE', stage / 'LICENSE')
        (stage / 'LICENSE').chmod(0o644)
        (stage / 'apps.yaml').write_text(
            'schema_version: 1\n'
            'app_id: ring\n'
            f'version: {version}\n'
            'command: ring\n'
            'targets:\n'
            f'  {apps_target}:\n'
            '    binary: bin/ring\n'
        )

        validation = silicon_apps('validate', str(stage))
        output = args.out / f'ring-apps-{version}-{apps_target}.tar.gz'
        output.unlink(missing_ok=True)
        packed = silicon_apps('pack', str(stage), '--output', str(output))
        digest = hashlib.sha256(output.read_bytes()).hexdigest()
        entry = {'target': apps_target, 'rust_target': rust_target, 'version': version, 'archive': str(output),
                 'sha256': digest, 'size': output.stat().st_size, 'binary_sha256': hashlib.sha256(binary.read_bytes()).hexdigest(),
                 'validate': validation, 'pack': packed}
        if sandbox and apps_target == 'linux-aarch64':
            entry['sandbox'] = sandbox_check(stage.resolve())
        packages.append(entry)
        print(f'{output.name}  {digest}', file=sys.stderr)

    (args.out / 'SHA256SUMS').write_text(''.join(f"{p['sha256']}  {pathlib.Path(p['archive']).name}\n" for p in packages))
    (args.out / 'packages.json').write_text(json.dumps({'app_id': 'ring', 'version': version, 'packages': packages}, indent=2) + '\n')
    print(json.dumps({'version': version, 'packages': [{k: p[k] for k in ('target', 'archive', 'sha256', 'size')} | (
        {'sandbox': p['sandbox']} if 'sandbox' in p else {}) for p in packages]}, indent=2))


if __name__ == '__main__':
    main()
