#!/usr/bin/env python3
"""Write SHA256SUMS and build.json next to the server binaries in a dist directory."""

import argparse
import hashlib
import json
import pathlib
import re
import subprocess

ROOT = pathlib.Path(__file__).resolve().parent.parent
BINARIES = ('ring-server', 'ring-media')


def git(*args: str) -> str:
    try:
        return subprocess.run(['git', '-C', str(ROOT), *args], check=True, capture_output=True, text=True).stdout.strip()
    except (OSError, subprocess.CalledProcessError):
        return ''


def main() -> None:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('dist', type=pathlib.Path)
    parser.add_argument('--rustc', default='')
    args = parser.parse_args()

    sums = {}
    for name in BINARIES:
        sums[name] = hashlib.sha256((args.dist / name).read_bytes()).hexdigest()
    (args.dist / 'SHA256SUMS').write_text(''.join(f'{digest}  {name}\n' for name, digest in sums.items()))

    manifest = (ROOT / 'Cargo.toml').read_text()
    version = re.search(r'^\[workspace\.package\][^\[]*?^version\s*=\s*"([^"]+)"', manifest, re.M | re.S).group(1)
    receipt = {
        'version': version,
        'source_revision': git('rev-parse', 'HEAD') or 'unknown',
        'source_dirty': bool(git('status', '--porcelain', '--', 'crates', 'Cargo.toml', 'Cargo.lock')),
        'target': 'aarch64-unknown-linux-gnu',
        'platform': 'amazonlinux:2023',
        'rustc': args.rustc,
        'sha256': sums,
    }
    (args.dist / 'build.json').write_text(json.dumps(receipt, indent=2) + '\n')


if __name__ == '__main__':
    main()
