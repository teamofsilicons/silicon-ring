#!/usr/bin/env python3
"""Check release signatures and six-target archive layout with inert fixture binaries."""
import argparse, base64, hashlib, importlib.util, json, os, pathlib, subprocess, tarfile, tempfile
source=pathlib.Path(__file__).with_name('package-release.py')
spec=importlib.util.spec_from_file_location('package_release',source)
module=importlib.util.module_from_spec(spec);spec.loader.exec_module(module)
with tempfile.TemporaryDirectory(prefix='ring-package-test-') as temp:
    root=pathlib.Path(temp);binaries=root/'binaries';binaries.mkdir()
    for platform,arch in module.TARGETS:
        (binaries/(f'ring-{platform}-{arch}'+('.exe' if platform=='windows' else ''))).write_bytes(b'fixture-not-a-production-binary\n')
    # This deterministic key is used only for a disposable packaging test.
    os.environ['RING_RELEASE_SIGNING_KEY']='00'*32
    args=argparse.Namespace(binary=binaries/'ring-linux-x86_64',platform='linux',arch='x86_64',version='0.0.0-test',base_url='https://example.invalid/releases',output=root/'dist',unsigned=False)
    metadata=module.package(args)
    message='{version}\n{protocol_major}\n{platform}\n{arch}\n{sha256}\n{url}'.format(**metadata).encode()
    (root/'public.der').write_bytes(bytes.fromhex('302a300506032b6570032100'+metadata['signing_public_key']))
    (root/'message').write_bytes(message);(root/'signature').write_bytes(base64.b64decode(metadata['signature']))
    command=['openssl','pkeyutl','-verify','-rawin','-pubin','-keyform','DER','-inkey',str(root/'public.der'),'-in',str(root/'message'),'-sigfile',str(root/'signature')]
    subprocess.run(command,check=True,stdout=subprocess.DEVNULL,stderr=subprocess.DEVNULL)
    (root/'message').write_bytes(message+b'tampered')
    assert subprocess.run(command,stdout=subprocess.DEVNULL,stderr=subprocess.DEVNULL).returncode != 0
    args.binary=binaries
    archive=module.honeycomb_package(args)
    with tarfile.open(archive) as packaged:
        members=packaged.getmembers();names=[m.name for m in members]
        assert len(names)==len(set(names)) and 'honeycomb.yaml' in names
        assert all(m.name=='honeycomb.yaml' or m.name=='targets' or m.name.startswith('targets/') for m in members)
        assert all(not m.issym() and not m.islnk() and '..' not in pathlib.PurePosixPath(m.name).parts for m in members)
        for platform,arch in module.TARGETS:
            binary=packaged.getmember(f'targets/{platform}-{arch}/ring'+('.exe' if platform=='windows' else ''))
            assert binary.mode & 0o111
    print('Release signature, tamper rejection and six-target Honeycomb layout checks passed')
