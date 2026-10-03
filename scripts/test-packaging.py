#!/usr/bin/env python3
"""Check signatures/layout; --ring-binary also runs the host setup with a clean PATH."""
import argparse, base64, hashlib, importlib.util, json, os, pathlib, shutil, subprocess, tarfile, tempfile, time
parser=argparse.ArgumentParser(description=__doc__)
parser.add_argument('--ring-binary',type=pathlib.Path,help='Native CLI binary for setup, config-preservation and daemon checks')
test_args=parser.parse_args()
source=pathlib.Path(__file__).with_name('package-release.py')
spec=importlib.util.spec_from_file_location('package_release',source)
module=importlib.util.module_from_spec(spec);spec.loader.exec_module(module)
with tempfile.TemporaryDirectory(prefix='ring-package-test-') as temp:
    root=pathlib.Path(temp);binaries=root/'binaries';binaries.mkdir()
    os.environ.update(SILICON_HOME=str(root/'honeycomb-home'),HONEYCOMB_AUTO_UPDATE='false',HONEYCOMB_NO_SERVICE='1',HONEYCOMB_TELEMETRY='false',HONEYCOMB_NO_MODIFY_PATH='1')
    for platform,arch in module.TARGETS:
        (binaries/(f'ring-{platform}-{arch}'+('.exe' if platform=='windows' else ''))).write_bytes(b'fixture-not-a-production-binary\n')
    if test_args.ring_binary:
        import platform as host_platform
        host={'Darwin':'macos','Linux':'linux','Windows':'windows'}[host_platform.system()]
        host_arch={'arm64':'aarch64','amd64':'x86_64'}.get(host_platform.machine().lower(),host_platform.machine().lower())
        executable='ring.exe' if host=='windows' else 'ring'
        shutil.copyfile(test_args.ring_binary,binaries/(f'ring-{host}-{host_arch}'+('.exe' if host=='windows' else '')))
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
            if os.name!='nt':
                assert binary.mode & 0o111
        if test_args.ring_binary:
            payload=root/'package path with spaces';payload.mkdir()
            setup='setup.ps1' if host=='windows' else 'setup.sh'
            for name in (executable,setup):
                member=packaged.getmember(f'targets/{host}-{host_arch}/{name}')
                (payload/name).write_bytes(packaged.extractfile(member).read())
                (payload/name).chmod(0o755)
    if test_args.ring_binary:
        # The install script must find its own binary before Honeycomb configures PATH.
        clean_path=root/'empty-path';clean_path.mkdir()
        if host=='windows':
            windows=pathlib.Path(os.environ['SystemRoot'])/'System32'
            powershell=windows/'WindowsPowerShell'/'v1.0'/'powershell.exe'
            path=os.pathsep.join((str(windows),str(powershell.parent)))
            command=[str(powershell),'-NoProfile','-NonInteractive','-ExecutionPolicy','Bypass','-File',str(payload/setup)]
        else:
            path=str(clean_path)
            command=[str(payload/setup)]
        assert shutil.which('ring',path=path) is None
        assert shutil.which('ring.exe',path=path) is None
        for configured in (False,True):
            home=root/('configured-home' if configured else 'fresh-home');home.mkdir()
            env={k:v for k,v in os.environ.items() if not k.startswith('SILICON_RING_') and k not in ('SILICON_ORG','ISI','PSModulePath')}
            env.update(SILICON_HOME=str(home),PATH=path)
            def cli(*args):
                result=subprocess.run([str(payload/executable),'--json',*args],env=env,capture_output=True,text=True,timeout=30)
                assert result.returncode==0,(args,result.stdout,result.stderr)
                return json.loads(result.stdout)
            config=home/'.ring'/'config.json'
            if configured:
                cli('config','set','--scope','local','{"server_url":"ws://127.0.0.1:8765/ws","telemetry.enabled":false}')
            before=config.read_bytes() if config.exists() else None
            try:
                result=subprocess.run(command,cwd=root,env=env,capture_output=True,text=True,timeout=30)
                assert result.returncode==0,(result.stdout,result.stderr)
                assert cli('daemon','status')['running'] is True
                values=cli('config','show','--scope','local')
                assert values['defaults']['server_url']=='wss://backend.ring.teamofsilicons.com/ws'
                assert (config.read_bytes() if config.exists() else None)==before
                if configured:
                    assert values['values']['server_url']=='ws://127.0.0.1:8765/ws'
            finally:
                cli('daemon','stop')
                for _ in range(100):
                    if not list((home/'.ring').glob('*-daemon.json')): break
                    time.sleep(.05)
                assert not list((home/'.ring').glob('*-daemon.json')),'Temporary daemon did not stop'
                assert cli('daemon','status')['running'] is False
        print('Clean-PATH Honeycomb setup, production default, preserved local settings and daemon cleanup passed')
    print('Release signature, tamper rejection and six-target Honeycomb layout checks passed')
