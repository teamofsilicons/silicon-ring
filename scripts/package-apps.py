#!/usr/bin/env python3
"""Build the Silicon Apps packages for the ring CLI from scripts/build-cli.sh output.

Silicon Apps is for silicons, so every package is the ring CLI: one per target with a live
validation runner (the four Linux targets and windows-x86_64), each a directory with apps.yaml,
bin/ring (bin/ring.exe on Windows) and LICENSE, checked with `silicon-apps validate` and packed with
`silicon-apps pack` (both local) into dist/apps/ring-apps-<version>-<target>.tar.gz, plus
dist/apps/SHA256SUMS and dist/apps/packages.json for scripts/publish-apps.py.

Where Ring has a desktop app (linux-x86_64, linux-aarch64, windows-x86_64), the package also
carries it for the people (carbons) on the same machine, with an install script that Silicon Apps
runs after installing or updating: scripts/apps-install/linux.sh (the AppImage, extracted per
user) or windows.cmd and windows.ps1 (the self-contained build, per user). They skip headless
machines and never fail the CLI install. The desktop builds come from --desktop (default
dist/desktop: the GitHub release's AppImages and Ring-windows-x64.zip, checked against its
SHA256SUMS).

When a docker host can run a target's architecture, its package is also run the way the Apps
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
import zipfile

ROOT = pathlib.Path(__file__).resolve().parent.parent
TARGETS = {
    'linux-x86_64': 'x86_64-unknown-linux-musl',
    'linux-aarch64': 'aarch64-unknown-linux-musl',
    'linux-i686': 'i686-unknown-linux-musl',
    'linux-armv7hf': 'armv7-unknown-linux-musleabihf',
    'windows-x86_64': 'x86_64-pc-windows-msvc',
}
ELF_MACHINE = {'linux-x86_64': b'\x3e\x00', 'linux-aarch64': b'\xb7\x00', 'linux-i686': b'\x03\x00',
               'linux-armv7hf': b'\x28\x00'}
PE_MACHINE = {'windows-x86_64': b'\x64\x86'}
DOCKER_PLATFORM = {'linux-x86_64': 'linux/amd64', 'linux-aarch64': 'linux/arm64', 'linux-i686': 'linux/386',
                   'linux-armv7hf': 'linux/arm/v7'}
# Static binaries need nothing from the image; it only has to exist for every platform.
VALIDATOR_IMAGE = 'busybox:1.37'
INSTALL = ROOT / 'scripts' / 'apps-install'


def desktop_payload(apps_target, desktop_version):
    """(file in --desktop, path in the package, install script files) for a target with a desktop app."""
    return {
        'linux-x86_64': (f'Ring-{desktop_version}-x86_64.AppImage', 'desktop/Ring.AppImage',
                         {'scripts/install.sh': INSTALL / 'linux.sh'}),
        'linux-aarch64': (f'Ring-{desktop_version}-aarch64.AppImage', 'desktop/Ring.AppImage',
                          {'scripts/install.sh': INSTALL / 'linux.sh'}),
        'windows-x86_64': ('Ring-windows-x64.zip', 'desktop/Ring-windows-x64.zip',
                           {'scripts/install.cmd': INSTALL / 'windows.cmd',
                            'scripts/install-desktop.ps1': INSTALL / 'windows.ps1'}),
    }.get(apps_target)


def fail(message):
    raise SystemExit(f'package-apps: {message}')


def workspace_version():
    manifest = (ROOT / 'Cargo.toml').read_text()
    match = re.search(r'^\[workspace\.package\][^\[]*?^version\s*=\s*"([^"]+)"', manifest, re.M | re.S)
    if not match:
        fail('no [workspace.package] version in Cargo.toml')
    return match.group(1)


def cli_version():
    """The CLI's own version (crates/ring-cli/Cargo.toml), else the workspace's."""
    manifest = (ROOT / 'crates' / 'ring-cli' / 'Cargo.toml').read_text()
    match = re.search(r'^\[package\][^\[]*?^version\s*=\s*"([^"]+)"', manifest, re.M | re.S)
    return match.group(1) if match else workspace_version()


def checksums(path):
    sums = {}
    for line in path.read_text().splitlines():
        digest, _, name = line.partition('  ')
        sums[name.strip().lstrip('*')] = digest.strip()
    return sums


def extract_binary(archive, destination):
    if archive.suffix == '.zip':
        with zipfile.ZipFile(archive) as zip_file:
            name = next((n for n in zip_file.namelist() if pathlib.PurePosixPath(n).name == 'ring.exe'), None)
            if name is None:
                fail(f'{archive.name} has no ring.exe')
            destination.write_bytes(zip_file.read(name))
    else:
        with tarfile.open(archive, 'r:gz') as tar:
            member = next((m for m in tar.getmembers() if m.isfile() and pathlib.PurePosixPath(m.name).name == 'ring'), None)
            if member is None:
                fail(f'{archive.name} has no ring binary')
            destination.write_bytes(tar.extractfile(member).read())
    destination.chmod(0o755)


def check_binary(binary, apps_target):
    data = binary.read_bytes()
    if apps_target in ELF_MACHINE:
        if data[:4] != b'\x7fELF' or data[18:20] != ELF_MACHINE[apps_target]:
            fail(f'{binary} is not a {apps_target} ELF')
    else:
        pe = int.from_bytes(data[0x3c:0x40], 'little')
        if data[:2] != b'MZ' or data[pe:pe + 4] != b'PE\0\0' or data[pe + 4:pe + 6] != PE_MACHINE[apps_target]:
            fail(f'{binary} is not a {apps_target} executable')


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


def can_run(platform):
    """Whether the docker host runs this platform's binaries (natively or emulated)."""
    run = subprocess.run(['docker', 'run', '--rm', '--platform', platform, VALIDATOR_IMAGE, 'true'],
                         text=True, capture_output=True, timeout=600)
    return run.returncode == 0


def sandbox_check(stage, platform, version):
    """Run the validator's three commands the way its Linux runner does."""
    base = ['docker', 'run', '--rm', '--platform', platform, '--network=none', '--read-only', '--cap-drop=ALL',
            '--security-opt=no-new-privileges', '--pids-limit=32', '--memory=256m', '--cpus=1', '--user=65534:65534',
            '--tmpfs=/tmp:rw,noexec,nosuid,size=16m', '-e', 'HOME=/tmp', '-e', 'SILICON_HOME=/tmp',
            '-v', f'{stage}:/package:ro', '-w', '/package', '--entrypoint', '/package/bin/ring', VALIDATOR_IMAGE]
    results = {}
    for command in (['--help'], ['accounts', '--json'], ['login', 'status', '--json']):
        run = subprocess.run(base + command, text=True, capture_output=True, timeout=120)
        name = ' '.join(command)
        if run.returncode != 0:
            fail(f'`ring {name}` exited {run.returncode} in the validator sandbox ({platform}): {run.stderr.strip()[:400]}')
        if command[-1] == '--json':
            data = json.loads(run.stdout)
            if command[0] == 'accounts' and data.get('app_id') != 'ring':
                fail('`ring accounts --json` does not report app_id "ring"')
            if command[0] == 'accounts' and data.get('version') != version:
                fail(f'the packaged ring reports version {data.get("version")!r}, not {version}; rebuild it')
            if command[0] == 'login' and data.get('authenticated') is not False:
                fail('`ring login status --json` is not authenticated:false when signed out')
        elif not (run.stdout + run.stderr).strip():
            fail('`ring --help` printed nothing')
        results[name] = 'passed'
    return results


def copy_file(source, destination, mode, crlf=False):
    destination.parent.mkdir(parents=True, exist_ok=True)
    data = source.read_bytes()
    if crlf:
        data = data.replace(b'\r\n', b'\n').replace(b'\n', b'\r\n')
    destination.write_bytes(data)
    destination.chmod(mode)


def main():
    parser = argparse.ArgumentParser(description=__doc__, formatter_class=argparse.RawDescriptionHelpFormatter)
    parser.add_argument('--cli', type=pathlib.Path, default=ROOT / 'dist' / 'cli', help='scripts/build-cli.sh output')
    parser.add_argument('--desktop', type=pathlib.Path, default=ROOT / 'dist' / 'desktop',
                        help='the desktop builds and their SHA256SUMS, from the GitHub release')
    parser.add_argument('--out', type=pathlib.Path, default=ROOT / 'dist' / 'apps')
    parser.add_argument('--target', action='append', choices=sorted(TARGETS), help='only these Apps targets')
    parser.add_argument('--no-sandbox', action='store_true', help='skip the docker validator runs')
    args = parser.parse_args()

    if not shutil.which('silicon-apps'):
        fail('silicon-apps is not installed')
    version = cli_version()
    desktop_version = workspace_version()
    sums = checksums(args.cli / 'SHA256SUMS')
    desktop_sums = checksums(args.desktop / 'SHA256SUMS') if (args.desktop / 'SHA256SUMS').is_file() else {}
    stage_root = args.out / 'stage'
    shutil.rmtree(stage_root, ignore_errors=True)
    args.out.mkdir(parents=True, exist_ok=True)
    docker = not args.no_sandbox and docker_arch() is not None
    # build-cli.sh clears the archives it does not build, so every target is rebuilt in one run.
    rebuild = 'scripts/build-cli.sh ' + ' '.join(TARGETS[t] for t in (args.target or sorted(TARGETS)))

    packages = []
    for apps_target in args.target or sorted(TARGETS):
        rust_target = TARGETS[apps_target]
        windows = apps_target.startswith('windows-')
        archive = args.cli / f'ring-{rust_target}.{"zip" if windows else "tar.gz"}'
        if not archive.is_file():
            fail(f'missing {archive}; run {rebuild}')
        if hashlib.sha256(archive.read_bytes()).hexdigest() != sums.get(archive.name):
            fail(f'{archive.name} does not match {args.cli}/SHA256SUMS')

        stage = stage_root / f'ring-{apps_target}'
        executable = 'bin/ring.exe' if windows else 'bin/ring'
        (stage / 'bin').mkdir(parents=True)
        binary = stage / executable
        extract_binary(archive, binary)
        check_binary(binary, apps_target)
        # The user agent (commands/mod.rs) is the one place the CLI's own version is written whole.
        if f'ring-cli/{version} ('.encode() not in binary.read_bytes():
            fail(f'{archive.name} was not built as ring {version}; run {rebuild}')
        copy_file(ROOT / 'LICENSE', stage / 'LICENSE', 0o644)
        target_lines = f'  {apps_target}:\n    binary: {executable}\n'

        desktop = desktop_payload(apps_target, desktop_version)
        if desktop:
            source_name, packaged, scripts = desktop
            source = args.desktop / source_name
            if not source.is_file():
                fail(f'missing {source}; download it from the GitHub release with its SHA256SUMS')
            if hashlib.sha256(source.read_bytes()).hexdigest() != desktop_sums.get(source_name):
                fail(f'{source_name} does not match {args.desktop}/SHA256SUMS')
            copy_file(source, stage / packaged, 0o755 if source_name.endswith('.AppImage') else 0o644)
            for path, script in scripts.items():
                copy_file(script, stage / path, 0o755 if path.endswith('.sh') else 0o644, crlf=windows)
            entry_script = next(iter(scripts))
            target_lines += f'    install_script: {entry_script}\n'

        (stage / 'apps.yaml').write_text(
            'schema_version: 1\n'
            'app_id: ring\n'
            f'version: {version}\n'
            'command: ring\n'
            'targets:\n'
            + target_lines
        )

        validation = silicon_apps('validate', str(stage))
        output = args.out / f'ring-apps-{version}-{apps_target}.tar.gz'
        output.unlink(missing_ok=True)
        packed = silicon_apps('pack', str(stage), '--output', str(output))
        digest = hashlib.sha256(output.read_bytes()).hexdigest()
        entry = {'target': apps_target, 'rust_target': rust_target, 'version': version, 'archive': str(output),
                 'sha256': digest, 'size': output.stat().st_size, 'binary_sha256': hashlib.sha256(binary.read_bytes()).hexdigest(),
                 'desktop': (desktop[0] if desktop else None), 'validate': validation, 'pack': packed}
        platform = DOCKER_PLATFORM.get(apps_target)
        if docker and platform and can_run(platform):
            entry['sandbox'] = sandbox_check(stage.resolve(), platform, version)
        packages.append(entry)
        print(f'{output.name}  {digest}', file=sys.stderr)

    (args.out / 'SHA256SUMS').write_text(''.join(f"{p['sha256']}  {pathlib.Path(p['archive']).name}\n" for p in packages))
    (args.out / 'packages.json').write_text(json.dumps({'app_id': 'ring', 'version': version, 'packages': packages}, indent=2) + '\n')
    print(json.dumps({'version': version, 'packages': [{k: p[k] for k in ('target', 'archive', 'sha256', 'size', 'desktop')} | (
        {'sandbox': p['sandbox']} if 'sandbox' in p else {}) for p in packages]}, indent=2))


if __name__ == '__main__':
    main()
