#!/usr/bin/env python3
"""Write a deterministic .tar.gz or .zip from SOURCE=NAME pairs.

  scripts/make-archive.py dist/cli/ring-x86_64-unknown-linux-musl.tar.gz path/to/ring=ring LICENSE=LICENSE

Entries are sorted, owned by root, dated SOURCE_DATE_EPOCH (or 2026-01-01), and get mode 0755 when
the source is executable or ends in .exe, 0644 otherwise. Used by build-cli.sh and the release workflow.
"""

import gzip
import io
import os
import pathlib
import sys
import tarfile
import time
import zipfile

EPOCH = int(os.environ.get('SOURCE_DATE_EPOCH', '1767225600'))


def entries(pairs):
    result = []
    for pair in pairs:
        source, sep, name = pair.partition('=')
        if not sep or not name or name.startswith('/') or '..' in pathlib.PurePosixPath(name).parts:
            raise SystemExit(f'bad entry {pair!r}; expected SOURCE=NAME with a relative NAME')
        path = pathlib.Path(source)
        if not path.is_file():
            raise SystemExit(f'{source} is not a file')
        executable = name.endswith('.exe') or (os.name != 'nt' and os.access(path, os.X_OK))
        result.append((name, path.read_bytes(), 0o755 if executable else 0o644))
    return sorted(result)


def write_tar(output, files):
    buffer = io.BytesIO()
    with tarfile.open(fileobj=buffer, mode='w', format=tarfile.PAX_FORMAT) as tar:
        for name, data, mode in files:
            info = tarfile.TarInfo(name)
            info.size, info.mode, info.mtime = len(data), mode, EPOCH
            info.uid = info.gid = 0
            info.uname = info.gname = 'root'
            tar.addfile(info, io.BytesIO(data))
    with open(output, 'wb') as raw, gzip.GzipFile(fileobj=raw, mode='wb', mtime=0, filename='') as gz:
        gz.write(buffer.getvalue())


def write_zip(output, files):
    stamp = time.gmtime(max(EPOCH, 315532800))[:6]
    with zipfile.ZipFile(output, 'w', compression=zipfile.ZIP_DEFLATED, compresslevel=9) as archive:
        for name, data, mode in files:
            info = zipfile.ZipInfo(name, date_time=stamp)
            info.external_attr = (0o100000 | mode) << 16
            info.compress_type = zipfile.ZIP_DEFLATED
            archive.writestr(info, data)


def main():
    if len(sys.argv) < 3:
        raise SystemExit(__doc__)
    output = pathlib.Path(sys.argv[1])
    files = entries(sys.argv[2:])
    output.parent.mkdir(parents=True, exist_ok=True)
    if output.name.endswith('.tar.gz'):
        write_tar(output, files)
    elif output.suffix == '.zip':
        write_zip(output, files)
    else:
        raise SystemExit('the output must end in .tar.gz or .zip')
    print(output)


if __name__ == '__main__':
    main()
