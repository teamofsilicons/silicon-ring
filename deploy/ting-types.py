#!/usr/bin/env python3
"""Register Ring's Ting notification types from deploy/ting-types.json, as an author of `ring`.

  deploy/ting-types.py                       show what would change (reads the current types only)
  deploy/ting-types.py --apply               register new types and update changed descriptions
  deploy/ting-types.py --apply --retire      also mark ring.* types that 1.0 never sends as retired

Needs a Ting session for si:tos:
  silicon-accounts login --app ting -q | ting login --token-stdin

Type names are permanent in Ting (there is no delete), so types from older Ring versions can only
be re-described. Registering an existing type with the same description is a no-op.
"""

import argparse
import json
import pathlib
import subprocess
import sys

TYPES = pathlib.Path(__file__).resolve().parent / 'ting-types.json'
APP = 'ring'
RETIRED = 'Retired in Silicon Ring 1.0. Ring no longer sends this notification.'


def fail(message):
    raise SystemExit(f'ting-types: {message}')


def ting(*args):
    result = subprocess.run(['ting', *args, '--json'], text=True, capture_output=True)
    if result.returncode != 0:
        fail(f'ting {" ".join(args[:2])} failed: {(result.stderr or result.stdout).strip()[:400]}')
    return json.loads(result.stdout) if result.stdout.strip() else {}


def current_types():
    found, cursor = {}, None
    while True:
        args = ['types', 'list', '--app', APP, '--limit', '100']
        if cursor:
            args += ['--cursor', cursor]
        page = ting(*args)
        for item in page.get('items', []):
            found[item['type']] = item.get('description', '')
        cursor = page.get('next_cursor')
        if not cursor:
            return found


def main():
    parser = argparse.ArgumentParser(description=__doc__, formatter_class=argparse.RawDescriptionHelpFormatter)
    parser.add_argument('--apply', action='store_true', help='make the changes (otherwise only show them)')
    parser.add_argument('--retire', action='store_true', help='re-describe ring.* types that 1.0 does not send')
    args = parser.parse_args()

    wanted = json.loads(TYPES.read_text())
    for name, description in wanted.items():
        if not name.startswith(f'{APP}.') or len(name.split('.')) != 3 or not 0 < len(description.encode()) <= 1000:
            fail(f'{name}: names are ring.<service>.<event> and descriptions 1-1000 bytes')

    existing = current_types()
    plan = {
        'register': sorted(n for n in wanted if n not in existing),
        'update': sorted(n for n in wanted if n in existing and existing[n] != wanted[n]),
        'unchanged': sorted(n for n in wanted if existing.get(n) == wanted[n]),
        'legacy': sorted(n for n in existing if n not in wanted),
    }
    retire = [n for n in plan['legacy'] if existing[n] != RETIRED] if args.retire else []
    print(json.dumps({**plan, 'retire': retire}, indent=2))
    if not args.apply:
        print('dry run: nothing changed (pass --apply)', file=sys.stderr)
        return 0

    for name in plan['register']:
        ting('types', 'register', '--type', name, '--description', wanted[name])
        print(f'registered {name}', file=sys.stderr)
    for name in plan['update']:
        ting('types', 'update', '--type', name, '--description', wanted[name])
        print(f'updated {name}', file=sys.stderr)
    for name in retire:
        ting('types', 'update', '--type', name, '--description', RETIRED)
        print(f'retired {name}', file=sys.stderr)
    return 0


if __name__ == '__main__':
    sys.exit(main())
