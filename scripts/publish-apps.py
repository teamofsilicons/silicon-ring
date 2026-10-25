#!/usr/bin/env python3
"""Publish the ring CLI packages from scripts/package-apps.py on Silicon Apps, as si:tos.

  scripts/publish-apps.py plan        print the exact command sequence; touches nothing
  scripts/publish-apps.py preflight   read-only checks: signed in as si:tos, Linux runners live, app exists
  scripts/publish-apps.py upload      upload every package in dist/apps/packages.json (hosted validation)
  scripts/publish-apps.py release     create the development release from the accepted packages
  scripts/publish-apps.py promote     promote that release to production with the same version
  scripts/publish-apps.py readiness   read-only review of what publishing still needs
  scripts/publish-apps.py publish     make the app live (no review gate)
  scripts/publish-apps.py verify      read-only: the store record and the signed production packages
  scripts/publish-apps.py all         upload, release, promote, readiness, publish, verify

Every mutation gets an idempotency key created once and stored 0600 in
secrets/platform/apps-<version>-<step>-key.txt, next to the exact request (…-request.json) and the
CLI's output (…-result.json). A finished step is not repeated. A step that ended uncertainly is
retried with the same key and the same arguments, which Apps answers with the original result;
different arguments under a recorded key are refused here instead of by the server (409).
"""

import argparse
import json
import os
import pathlib
import shlex
import subprocess
import sys
import uuid

ROOT = pathlib.Path(__file__).resolve().parent.parent
STATE = ROOT / 'secrets' / 'platform'
PACKAGES = ROOT / 'dist' / 'apps' / 'packages.json'
APP = 'ring'
# Linux packages are checked by Apps' hosted runners. Windows and macOS packages are uploaded with
# --confirm-platform-checks, which says the three commands were run on that platform: package-apps.py
# records that run (native_check) for macOS on an Apple silicon Mac, and windows-x86_64 here is the
# package Apps' own Windows runner accepted on 2026-10-25, before it moved to confirmations.
SERVER_TARGETS = ('linux-x86_64', 'linux-aarch64', 'linux-i686', 'linux-armv7hf')
CONFIRMED_TARGETS = ('windows-x86_64', 'macos-aarch64')
LIVE_TARGETS = SERVER_TARGETS + CONFIRMED_TARGETS


def fail(message):
    raise SystemExit(f'publish-apps: {message}')


def load_packages():
    if not PACKAGES.is_file():
        fail(f'{PACKAGES.relative_to(ROOT)} not found; run scripts/build-cli.sh and scripts/package-apps.py')
    data = json.loads(PACKAGES.read_text())
    return data['version'], data['packages']


def private_write(path, text):
    fd = os.open(path, os.O_WRONLY | os.O_CREAT | os.O_TRUNC, 0o600)
    with os.fdopen(fd, 'w') as handle:
        handle.write(text)


def idempotency_key(version, step):
    path = STATE / f'apps-{version}-{step}-key.txt'
    try:
        fd = os.open(path, os.O_WRONLY | os.O_CREAT | os.O_EXCL, 0o600)
    except FileExistsError:
        return path.read_text().strip()
    key = str(uuid.uuid4())
    with os.fdopen(fd, 'w') as handle:
        handle.write(key + '\n')
    return key


def cli(*args):
    env = {**os.environ, 'SILICON_APPS_NO_DAEMON': '1'}
    return subprocess.run(['silicon-apps', *args], text=True, capture_output=True, env=env)


def read_only(*args):
    result = cli(*args, '--json')
    if result.returncode != 0:
        fail(f'silicon-apps {" ".join(args)} failed: {result.stderr.strip() or result.stdout.strip()}')
    return json.loads(result.stdout) if result.stdout.strip() else {}


def mutation(version, step, args):
    """Run one recorded mutation and return its JSON result."""
    STATE.mkdir(mode=0o700, parents=True, exist_ok=True)
    result_path = STATE / f'apps-{version}-{step}-result.json'
    request_path = STATE / f'apps-{version}-{step}-request.json'
    if result_path.is_file():
        print(f'{step}: already done ({result_path.relative_to(ROOT)})', file=sys.stderr)
        return json.loads(result_path.read_text())
    key = idempotency_key(version, step)
    request = {'arguments': args, 'idempotency_key': key}
    if request_path.is_file() and json.loads(request_path.read_text()) != request:
        fail(f'{step} was attempted with other arguments; inspect {request_path.relative_to(ROOT)} before retrying')
    private_write(request_path, json.dumps(request, indent=2) + '\n')
    print(f'{step}: silicon-apps {shlex.join(args)} --idempotency-key <{key[:8]}…> --json', file=sys.stderr)
    run = cli(*args, '--idempotency-key', key, '--json')
    private_write(STATE / f'apps-{version}-{step}.stderr.txt', run.stderr)
    if run.returncode != 0:
        private_write(STATE / f'apps-{version}-{step}-failed.json', run.stdout)
        fail(f'{step} failed (exit {run.returncode}); output in secrets/platform/apps-{version}-{step}-failed.json. '
             'Retry the same command to reuse the key once the cause is understood.')
    private_write(result_path, run.stdout)
    return json.loads(run.stdout) if run.stdout.strip() else {}


def plan(version, packages):
    lines = [
        '# Read-only preflight',
        'silicon-apps login status --json',
        f'silicon-apps capabilities --require {",".join("target:" + t for t in SERVER_TARGETS)} --json',
        f'silicon-apps show {APP} --json',
        '# Mutations (each with its own stored --idempotency-key)',
    ]
    for p in packages:
        confirm = ' --confirm-platform-checks' if p['target'] in CONFIRMED_TARGETS else ''
        lines.append(f"silicon-apps upload {APP} --target {p['target']} {shlex.quote(p['archive'])}{confirm} --json")
    lines += [
        f'silicon-apps release {APP} --version {version} ' + ' '.join(f'--package <{p["target"]} package id>' for p in packages)
        + ' --notes "…" --json',
        f'silicon-apps promote {APP} <development release id> --version {version} --json',
        f'silicon-apps readiness {APP} --json',
        f'silicon-apps publish {APP} --json',
        '# Verify (read-only)',
        f'silicon-apps show {APP} --json',
    ] + [f'silicon-apps show {APP} --install-script --target {p["target"]} --json' for p in packages]
    print('\n'.join(lines))


def preflight():
    status = read_only('login', 'status')
    account = status.get('account') or {}
    if not status.get('authenticated') or account.get('id') != 'si:tos':
        fail('silicon-apps is not signed in as si:tos')
    read_only('capabilities', '--require', ','.join('target:' + t for t in SERVER_TARGETS))
    app = read_only('show', APP)
    print(json.dumps({'signed_in_as': account.get('id'), 'targets_live': list(LIVE_TARGETS),
                      'app': app.get('app_id'), 'published': app.get('published')}, indent=2))


def upload(version, packages, confirmed=None):
    """confirmed: {target: binary sha256} the operator ran the three commands of, on that platform."""
    confirmed = confirmed or {}
    ids = {}
    for p in packages:
        if p['target'] not in LIVE_TARGETS:
            print(f"skipping {p['target']}: no live validation runner", file=sys.stderr)
            continue
        args = ['upload', APP, '--target', p['target'], p['archive']]
        if p['target'] in CONFIRMED_TARGETS:
            done = (STATE / f"apps-{version}-upload-{p['target']}-result.json").is_file()
            checked = (p.get('native_check') and all(v == 'passed' for v in p['native_check'].values())) \
                or confirmed.get(p['target']) == p['binary_sha256']
            if not done and not checked:
                how = ('run scripts/package-apps.py on an Apple silicon Mac so it records native_check'
                       if p['target'].startswith('macos-') else
                       f"run its bin/ring --help, accounts --json and login status --json on {p['target']}, signed out "
                       f"with an empty SILICON_HOME, then pass --confirmed {p['target']}={p['binary_sha256']}")
                fail(f"{p['target']} has no hosted runner, and Apps takes our word for it (--confirm-platform-checks): {how}")
            args.append('--confirm-platform-checks')
        result = mutation(version, f"upload-{p['target']}", args)
        failed = [v for v in result.get('validation', []) if not v.get('passed')]
        if failed:
            fail(f"{p['target']} failed hosted validation: {json.dumps(failed)[:600]}")
        ids[p['target']] = result['id']
    return ids


def release(version, packages, notes, confirmed=None):
    ids = upload(version, packages, confirmed)
    args = ['release', APP, '--version', version]
    for target in sorted(ids):
        args += ['--package', str(ids[target])]
    args += ['--notes', notes]
    return mutation(version, 'release', args)


def promote(version, packages, notes, confirmed=None):
    development = release(version, packages, notes, confirmed)
    return mutation(version, 'promote', ['promote', APP, str(development['id']), '--version', version])


def readiness():
    result = read_only('readiness', APP)
    print(json.dumps(result, indent=2))
    if not result.get('ready'):
        fail('the app is not ready to publish')


def verify(packages):
    app = read_only('show', APP)
    production = (app.get('latest_production') or {}).get('version')
    print(json.dumps({'published': app.get('published'), 'targets': app.get('targets'), 'production': production}, indent=2))
    for p in packages:
        if p['target'] in LIVE_TARGETS:
            read_only('show', APP, '--install-script', '--target', p['target'])
    print('signatures verified for ' + ', '.join(p['target'] for p in packages if p['target'] in LIVE_TARGETS))


def main():
    parser = argparse.ArgumentParser(description=__doc__, formatter_class=argparse.RawDescriptionHelpFormatter)
    parser.add_argument('step', choices=['plan', 'preflight', 'upload', 'release', 'promote', 'readiness', 'publish',
                                         'verify', 'all'])
    parser.add_argument('--notes', help='release notes (default: "Silicon Ring <version>. Ring sends no telemetry.")')
    parser.add_argument('--confirmed', action='append', default=[], metavar='TARGET=BINARY_SHA256',
                        help='you ran --help, accounts --json and login status --json of this exact packaged binary on '
                             'TARGET, signed out with an empty SILICON_HOME (targets without a hosted runner)')
    args = parser.parse_args()
    version, packages = load_packages()
    confirmed = dict(c.split('=', 1) for c in args.confirmed)
    notes = args.notes or f'Silicon Ring {version}. Ring sends no telemetry.'

    if args.step == 'plan':
        plan(version, packages)
    elif args.step == 'preflight':
        preflight()
    elif args.step == 'upload':
        print(json.dumps(upload(version, packages, confirmed), indent=2))
    elif args.step == 'release':
        print(json.dumps(release(version, packages, notes, confirmed), indent=2))
    elif args.step == 'promote':
        print(json.dumps(promote(version, packages, notes, confirmed), indent=2))
    elif args.step == 'readiness':
        readiness()
    elif args.step == 'publish':
        readiness()
        print(json.dumps(mutation(version, 'publish', ['publish', APP]), indent=2))
    elif args.step == 'verify':
        verify(packages)
    else:
        preflight()
        promote(version, packages, notes, confirmed)
        readiness()
        mutation(version, 'publish', ['publish', APP])
        verify(packages)


if __name__ == '__main__':
    main()
