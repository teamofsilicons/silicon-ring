#!/usr/bin/env python3
"""Install a checksummed native release through SSM, with readiness rollback."""
import argparse
import datetime
import json
from pathlib import Path
import re
import subprocess
import tempfile
import textwrap
import time
import uuid

parser = argparse.ArgumentParser(description=__doc__)
for field in ["instance-id", "release-bucket", "release-key", "release-sha256", "secret-arn", "assets-bucket"]:
    parser.add_argument("--" + field, required=True)
parser.add_argument("--region", default="us-west-1")
args = parser.parse_args()
assert re.fullmatch(r"i-[0-9a-f]+", args.instance_id)
assert re.fullmatch(r"[a-z0-9.-]+", args.release_bucket)
assert re.fullmatch(r"[a-z0-9.-]+", args.assets_bucket)
assert re.fullmatch(r"[A-Za-z0-9._/-]+", args.release_key)
assert re.fullmatch(r"[a-f0-9]{64}", args.release_sha256)
assert re.fullmatch(r"arn:aws:secretsmanager:[a-z0-9-]+:[0-9]+:secret:[A-Za-z0-9/_+=.@-]+", args.secret_arn)
assert re.fullmatch(r"[a-z0-9-]+", args.region)

# Keep bootstrap security, paths, and environment serialization in one source.
template = Path("deploy/aws.yaml").read_text()
script = textwrap.dedent(template.split("Fn::Base64: !Sub |\n", 1)[1].split("\n  Address:\n", 1)[0])
for key, value in {
    "ReleaseBucket": args.release_bucket, "ReleaseKey": args.release_key,
    "ReleaseSha256": args.release_sha256, "SecretsArn": args.secret_arn,
    "Assets": args.assets_bucket, "AWS::Region": args.region,
}.items():
    script = script.replace("$" + "{" + key + "}", value)
if re.search(r"\$\{[^}]+\}", script):
    raise RuntimeError("Unresolved CloudFormation substitution in native bootstrap")
script = script.replace("test ! -e \"$release\"\nmv \"$staging\" \"$release\"", 'if test ! -e "$release"; then mv "$staging" "$release"; fi')
script = script.replace('ln -s "$release" /opt/ring/current', 'ln -sfn "$release" /opt/ring/current.next\nmv -Tf /opt/ring/current.next /opt/ring/current')
script = script.replace("systemctl enable --now ring.service", "systemctl enable ring.service\nsystemctl restart ring.service")
script = script.replace("install -d -m 700 -o ring -g ring /var/lib/ring", "install -d -m 700 -o ring -g ring /var/lib/ring\nchown -R ring:ring /var/lib/ring")
# Preserve the temporary physical-device route during a normal release update.
caddy_begin = script.index("caddy_url=")
caddy_end = script.index("/usr/local/bin/caddy validate", caddy_begin)
script = script[:caddy_begin] + "if test ! -f /etc/systemd/system/caddy.service; then\n" + script[caddy_begin:caddy_end] + "fi\nchmod 644 /etc/systemd/system/ring.service /etc/systemd/system/caddy.service\n" + script[caddy_end:]
probe = r'''
probe=$(mktemp -d /var/tmp/ring-native-probe.XXXXXXXX)
status=0
timeout 3 env -i PATH=/usr/local/bin:/usr/bin:/bin RING_ENV=test RING_BIND=127.0.0.1:0 RING_DATA_DIR="$probe" RING_DISABLE_PROVIDERS=1 RING_TELEMETRY_ENABLED=false "$staging/ring-server" > "$probe/output.log" 2>&1 || status=$?
if test "$status" != 124; then cat "$probe/output.log"; rm -rf "$probe"; exit 1; fi
rm -rf "$probe"
'''
script = script.replace("release='/opt/ring/releases/", probe + "\nrelease='/opt/ring/releases/", 1)
backup = "/etc/ring/server.env.rollback-" + uuid.uuid4().hex[:12]
preamble = r'''
cloud-init status --wait >/dev/null
# The old initial bootstrap used containers; explicitly stop that runtime.
if command -v docker >/dev/null 2>&1; then systemctl disable --now docker.socket docker.service || true; fi
previous=''
if test -L /opt/ring/current; then previous=$(readlink -f /opt/ring/current || true); fi
previous_env='BACKUP'
if test -f /etc/ring/server.env; then cp /etc/ring/server.env "$previous_env"; chmod 600 "$previous_env"; fi
rollback() {
  code=$?
  trap - ERR
  if test -n "$previous" && test -x "$previous/ring-server"; then
    ln -sfn "$previous" /opt/ring/current.rollback
    mv -Tf /opt/ring/current.rollback /opt/ring/current
    if test -f "$previous_env"; then mv "$previous_env" /etc/ring/server.env; fi
    systemctl restart ring.service || true
    echo 'Native update failed; previous release restored.' >&2
  else
    echo 'Native installation did not become ready.' >&2
  fi
  exit "$code"
}
trap rollback ERR
'''.replace("BACKUP", backup)
script = script.replace("set -euo pipefail", "set -Eeuo pipefail\n" + preamble, 1)
script += '\ntrap - ERR\nrm -f "$previous_env"\necho "Native Ring release is ready."\n'

def aws(*command):
    result = subprocess.run(["aws", *command, "--region", args.region, "--output", "json"], capture_output=True, text=True, check=True)
    return json.loads(result.stdout) if result.stdout.strip() else {}

with tempfile.NamedTemporaryFile("w", prefix="ring-native-update-", suffix=".json") as parameters:
    json.dump({"commands": [script], "executionTimeout": ["900"]}, parameters)
    parameters.flush()
    command = aws("ssm", "send-command", "--instance-ids", args.instance_id, "--document-name", "AWS-RunShellScript", "--comment", "Native Ring deployment with readiness rollback", "--parameters", "file://" + parameters.name)["Command"]["CommandId"]
print(json.dumps({"command_id": command, "instance_id": args.instance_id}), flush=True)
deadline = time.monotonic() + 960
while time.monotonic() < deadline:
    time.sleep(3)
    try:
        result = aws("ssm", "get-command-invocation", "--command-id", command, "--instance-id", args.instance_id)
    except subprocess.CalledProcessError as error:
        if "InvocationDoesNotExist" in error.stderr:
            continue
        raise
    if result["Status"] in ("Pending", "InProgress", "Delayed"):
        continue
    print(result["StandardOutputContent"][-3000:].strip())
    print(json.dumps({"status": result["Status"]}))
    if result["Status"] != "Success":
        print(result["StandardErrorContent"][-2000:])
    else:
        receipt = Path("deploy/aws-deployment.private.json")
        receipt.write_text(json.dumps({"instance_id": args.instance_id, "region": args.region, "release_bucket": args.release_bucket, "release_key": args.release_key, "release_sha256": args.release_sha256, "ssm_command_id": command, "deployed_at": datetime.datetime.now(datetime.timezone.utc).isoformat()}, indent=2) + "\n")
        receipt.chmod(0o600)
    raise SystemExit(0 if result["Status"] == "Success" else 1)
raise TimeoutError("Inspect SSM command " + command)
