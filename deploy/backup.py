#!/usr/bin/env python3
"""Back up and restore the Ring SQLite databases on the host.

  backup.py [--label L] [--local-only]   copy every database in /var/lib/ring with SQLite's online
                                         backup, check it, tar it and upload it to
                                         s3://<bucket>/backups/<label>-<host>-<UTC>.tar.gz
  backup.py snapshot --dest DIR          the same copy into DIR only (used before a release switch)
  backup.py restore --from S3URI|PATH    put a backup's databases back into an empty data directory

Runs as root under Python 3.9 (Amazon Linux 2023) with the standard library and the aws CLI, so it
also works on hosts that predate Ring 1.0. Prints paths and sizes, never data.
"""

import argparse
import datetime
import hashlib
import json
import os
import pathlib
import pwd
import shutil
import socket
import sqlite3
import subprocess
import sys
import tarfile
import tempfile

SUFFIXES = ('.sqlite3', '.sqlite', '.db')
LOCAL_DIR = pathlib.Path('/var/backups/ring')


def read_env_file(path):
    values = {}
    if path and path.exists():
        for line in path.read_text().splitlines():
            key, sep, value = line.partition('=')
            if sep and not key.startswith('#'):
                values[key.strip()] = value.strip().strip('"')
    return values


def databases(data_dir):
    return sorted(p for p in data_dir.rglob('*') if p.is_file() and p.suffix in SUFFIXES)


def copy_database(source, destination):
    destination.parent.mkdir(parents=True, exist_ok=True)
    with sqlite3.connect(f'file:{source}?mode=ro', uri=True) as db, sqlite3.connect(str(destination)) as copy:
        db.backup(copy)
        result = copy.execute('PRAGMA integrity_check').fetchone()[0]
    if result != 'ok':
        raise SystemExit(f'integrity check failed for the copy of {source.name}: {result}')
    destination.chmod(0o600)
    return {
        'path': str(source),
        'size': destination.stat().st_size,
        'sha256': hashlib.sha256(destination.read_bytes()).hexdigest(),
    }


def snapshot(data_dir, dest):
    found = databases(data_dir)
    manifest = {'created_at': datetime.datetime.now(datetime.timezone.utc).isoformat(), 'host': socket.gethostname(),
                'data_dir': str(data_dir), 'databases': []}
    for source in found:
        entry = copy_database(source, dest / source.relative_to(data_dir))
        entry['name'] = str(source.relative_to(data_dir))
        manifest['databases'].append(entry)
    (dest / 'manifest.json').write_text(json.dumps(manifest, indent=2) + '\n')
    return manifest


def aws(*args, region):
    result = subprocess.run(['aws', '--region', region, *args])
    if result.returncode != 0:
        raise SystemExit(f'aws {" ".join(args[:2])} failed with exit status {result.returncode}')


def prune_local(keep):
    archives = sorted(LOCAL_DIR.glob('*.tar.gz'), key=lambda p: p.stat().st_mtime, reverse=True)
    for old in archives[keep:]:
        old.unlink()


def cmd_backup(args):
    env = read_env_file(args.env_file)
    bucket = args.bucket or env.get('RING_S3_BUCKET')
    region = args.region or env.get('AWS_REGION') or 'us-west-1'
    if not bucket and not args.local_only:
        raise SystemExit('no bucket: pass --bucket or --env-file with RING_S3_BUCKET')
    LOCAL_DIR.mkdir(mode=0o700, parents=True, exist_ok=True)
    stamp = datetime.datetime.now(datetime.timezone.utc).strftime('%Y%m%dT%H%M%SZ')
    name = f'{args.label}-{socket.gethostname().split(".")[0]}-{stamp}.tar.gz'
    with tempfile.TemporaryDirectory() as tmp:
        manifest = snapshot(args.data_dir, pathlib.Path(tmp))
        archive = LOCAL_DIR / name
        with tarfile.open(archive, 'w:gz') as tar:
            for path in sorted(pathlib.Path(tmp).rglob('*')):
                tar.add(path, arcname=str(path.relative_to(tmp)), recursive=False)
        archive.chmod(0o600)
    result = {'archive': str(archive), 'databases': [{'name': d['name'], 'size': d['size']} for d in manifest['databases']]}
    if not args.local_only:
        uri = f's3://{bucket}/{args.prefix.strip("/")}/{name}'
        aws('s3', 'cp', str(archive), uri, '--sse', 'AES256', '--only-show-errors', region=region)
        result['s3_uri'] = uri
    prune_local(args.keep_local)
    print(json.dumps(result))


def cmd_snapshot(args):
    args.dest.mkdir(mode=0o700, parents=True, exist_ok=True)
    manifest = snapshot(args.data_dir, args.dest)
    print(json.dumps({'snapshot': str(args.dest), 'databases': [d['name'] for d in manifest['databases']]}))


def cmd_restore(args):
    existing = databases(args.data_dir)
    if existing and not args.replace:
        raise SystemExit(f'{args.data_dir} already has databases; pass --replace to overwrite them')
    region = args.region or 'us-west-1'
    with tempfile.TemporaryDirectory() as tmp:
        work = pathlib.Path(tmp)
        source = args.source
        if source.startswith('s3://'):
            aws('s3', 'cp', source, str(work / 'backup.tar.gz'), '--only-show-errors', region=region)
            source = str(work / 'backup.tar.gz')
        unpacked = work / 'unpacked'
        with tarfile.open(source, 'r:gz') as tar:
            for member in tar.getmembers():
                parts = pathlib.PurePosixPath(member.name).parts
                if member.name.startswith('/') or '..' in parts or not (member.isfile() or member.isdir()):
                    raise SystemExit('the backup contains an unexpected path')
            tar.extractall(unpacked)
        manifest = json.loads((unpacked / 'manifest.json').read_text())
        owner = pwd.getpwnam(args.owner)
        for entry in manifest['databases']:
            copy = unpacked / entry['name']
            if hashlib.sha256(copy.read_bytes()).hexdigest() != entry['sha256']:
                raise SystemExit(f'checksum mismatch for {entry["name"]}')
            target = args.data_dir / entry['name']
            target.parent.mkdir(parents=True, exist_ok=True)
            for suffix in ('-wal', '-shm', '-journal'):
                stale = target.with_name(target.name + suffix)
                if stale.exists():
                    stale.unlink()
            shutil.copy2(copy, target)
            os.chown(target, owner.pw_uid, owner.pw_gid)
            target.chmod(0o600)
            print(f'restored {entry["name"]} ({entry["size"]} bytes)')


def main():
    parser = argparse.ArgumentParser(description=__doc__, formatter_class=argparse.RawDescriptionHelpFormatter)
    parser.add_argument('--data-dir', type=pathlib.Path, default=pathlib.Path('/var/lib/ring'))
    parser.add_argument('--region')
    sub = parser.add_subparsers(dest='command')

    backup = sub.add_parser('backup')
    for target in (parser, backup):
        target.add_argument('--env-file', type=pathlib.Path, default=pathlib.Path('/etc/ring/server.env'))
        target.add_argument('--bucket')
        target.add_argument('--prefix', default='backups')
        target.add_argument('--label', default='manual')
        target.add_argument('--keep-local', type=int, default=7)
        target.add_argument('--local-only', action='store_true')

    snap = sub.add_parser('snapshot')
    snap.add_argument('--dest', type=pathlib.Path, required=True)

    restore = sub.add_parser('restore')
    restore.add_argument('--from', dest='source', required=True)
    restore.add_argument('--owner', default='ring')
    restore.add_argument('--replace', action='store_true')

    args = parser.parse_args()
    if args.command == 'snapshot':
        cmd_snapshot(args)
    elif args.command == 'restore':
        cmd_restore(args)
    else:
        cmd_backup(args)


if __name__ == '__main__':
    main()
