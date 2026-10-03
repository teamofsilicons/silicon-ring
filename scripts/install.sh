#!/bin/sh
# Installs technical runtime only. IAM authentication is always a separate step.
set -eu
: "${SILICON_HOME:?Set SILICON_HOME to the interpreter home before installing Ring}"
install_dir=${RING_INSTALL_PREFIX:-"$SILICON_HOME/.ring/install"}
remote=0
if [ "${RING_INSTALL_FROM_SOURCE:-0}" = 1 ] || { [ -f ./crates/ring-cli/Cargo.toml ] && [ -z "${RING_RELEASE_MANIFEST_URL:-}" ]; }; then
  command -v cargo >/dev/null 2>&1 || { echo 'Install Rust from https://rustup.rs, then retry.' >&2; exit 1; }
  if [ "$(uname -s)" = Linux ]; then
    if ! command -v pkg-config >/dev/null 2>&1 || ! pkg-config --exists alsa; then
      echo 'Install pkg-config and ALSA development headers (for example libasound2-dev).' >&2; exit 1
    fi
  fi
  source_dir=${RING_SOURCE_DIR:-.}
  cargo install --locked --path "$source_dir/crates/ring-cli" --root "$install_dir"
else
  remote=1
  command -v python3 >/dev/null 2>&1 || { echo 'Install Python 3, then retry.' >&2; exit 1; }
  command -v openssl >/dev/null 2>&1 || { echo 'Install OpenSSL 3 for Ed25519 verification, then retry.' >&2; exit 1; }
  export SILICON_RING_RELEASE_PUBLIC_KEY="${SILICON_RING_RELEASE_PUBLIC_KEY:-b6e640b29d9945186eda5dfeb50cf491d4e72a20aaefba6d18ddc22343b44a65}"
  export RING_RELEASE_MANIFEST_URL="${RING_RELEASE_MANIFEST_URL:-https://ring.teamofsilicons.com/releases/release-index.json}"
  python3 - "$install_dir" <<'PY'
import base64, hashlib, json, os, pathlib, platform, re, subprocess, sys, tempfile, urllib.request
install = pathlib.Path(sys.argv[1]).expanduser().resolve() / 'bin'
system = {'Darwin':'macos','Linux':'linux','Windows':'windows'}.get(platform.system())
arch = {'arm64':'aarch64','aarch64':'aarch64','x86_64':'x86_64','AMD64':'x86_64'}.get(platform.machine())
if not system or not arch: raise SystemExit('Unsupported platform or architecture')
key = os.environ['SILICON_RING_RELEASE_PUBLIC_KEY']
if not re.fullmatch(r'[0-9a-fA-F]{64}', key): raise SystemExit('The pinned Ed25519 public key must be 64 hex characters')
def fetch(url, limit):
    if not url.startswith('https://'): raise SystemExit('Release URLs must use HTTPS')
    with urllib.request.urlopen(url, timeout=120) as response:
        if not response.geturl().startswith('https://'): raise SystemExit('Unencrypted download redirect rejected')
        result = response.read(limit+1)
    if len(result)>limit: raise SystemExit('Release download exceeds its size limit')
    return result
metadata = json.loads(fetch(os.environ['RING_RELEASE_MANIFEST_URL'], 1024*1024))
releases = metadata.get('releases',[metadata])
release = next((r for r in releases if r.get('platform')==system and r.get('arch')==arch),None)
if not release or release.get('protocol_major')!=1: raise SystemExit('No compatible protocol-1 release for this machine')
message = '{version}\n{protocol_major}\n{platform}\n{arch}\n{sha256}\n{url}'.format(**release).encode()
with tempfile.TemporaryDirectory(prefix='ring-verify-') as temp:
    temp=pathlib.Path(temp)
    (temp/'key.der').write_bytes(bytes.fromhex('302a300506032b6570032100'+key))
    (temp/'message').write_bytes(message)
    (temp/'signature').write_bytes(base64.b64decode(release['signature'],validate=True))
    verified=subprocess.run(['openssl','pkeyutl','-verify','-rawin','-pubin','-keyform','DER','-inkey',str(temp/'key.der'),'-in',str(temp/'message'),'-sigfile',str(temp/'signature')],stdout=subprocess.DEVNULL,stderr=subprocess.PIPE)
    if verified.returncode: raise SystemExit('Signature verification failed. Check the pinned key and OpenSSL 3 installation; nothing was installed.')
    binary=fetch(release['url'],200*1024*1024)
    if hashlib.sha256(binary).hexdigest()!=release['sha256']: raise SystemExit('Signed SHA-256 mismatch; nothing was installed')
    install.mkdir(parents=True,exist_ok=True)
    destination=install/('ring.exe' if system=='windows' else 'ring')
    candidate=install/('.ring-verified-'+os.urandom(8).hex())
    try:
        candidate.write_bytes(binary)
        candidate.chmod(0o755)
        os.replace(candidate,destination)
    finally:
        candidate.unlink(missing_ok=True)
print('Verified and installed Ring '+release['version'])
PY
fi
ring_binary="$install_dir/bin/ring"
if [ -f "$install_dir/bin/ring.exe" ]; then ring_binary="$install_dir/bin/ring.exe"; fi
if [ "$remote" = 1 ]; then
  python3 - "$ring_binary" <<'PYCONFIG'
import json,os,pathlib,subprocess,sys
path=pathlib.Path(os.environ['SILICON_HOME'])/'.ring/config.json'
config=json.loads(path.read_text()) if path.exists() else {}
if 'server_url' not in config:
    subprocess.run([sys.argv[1],'config','set','--scope','local',json.dumps({'server_url':os.environ.get('RING_SERVER_URL','wss://backend.ring.teamofsilicons.com/ws')})],check=True,stdout=subprocess.DEVNULL)
PYCONFIG
fi
"$ring_binary" daemon start
printf '\nInstalled Ring at %s\nAdd %s/bin to PATH. Authenticate separately: ring login --token-stdin\n' "$ring_binary" "$install_dir"
printf 'Signed compatible updates are checked hourly. The daemon uses the deployment key from SILICON_RING_RELEASE_PUBLIC_KEY.\n'
