#!/usr/bin/env python3
"""Start the isolated native physical-device check endpoint on the Ring host."""
import argparse
import json
from pathlib import Path
import re
import shlex
import subprocess
import tempfile
import time
import uuid

parser = argparse.ArgumentParser(description=__doc__)
parser.add_argument("--instance-id", required=True)
parser.add_argument("--bucket", required=True)
parser.add_argument("--secret-arn", required=True)
parser.add_argument("--fixtures", type=Path, default=Path("deploy/devices-test.private.json"))
parser.add_argument("--region", default="us-west-1")
args = parser.parse_args()
assert re.fullmatch(r"i-[0-9a-f]+", args.instance_id)
assert re.fullmatch(r"[a-z0-9.-]+", args.bucket)
assert args.fixtures.stat().st_mode & 0o077 == 0
fixture = json.loads(args.fixtures.read_text())
assert fixture["identities"] and fixture["test_app_secret"]

def aws(*command):
    result = subprocess.run(["aws", *command, "--region", args.region, "--output", "json"], capture_output=True, text=True, check=True)
    return json.loads(result.stdout) if result.stdout.strip() else {}

key = "device-check/" + uuid.uuid4().hex + ".private.json"
aws("s3api", "put-object", "--bucket", args.bucket, "--key", key, "--body", str(args.fixtures), "--server-side-encryption", "AES256")
q = shlex.quote
script = r'''#!/bin/bash
set -euo pipefail
umask 077
temp=$(mktemp -d)
trap 'rm -rf "$temp"' EXIT
install -d -m 700 /etc/ring-devicecheck
install -d -m 700 -o ring -g ring /var/lib/ring-devicecheck
aws s3 cp @@FIXTURE@@ "$temp/fixture.json" --region @@REGION@@ --only-show-errors
aws secretsmanager get-secret-value --secret-id @@SECRET@@ --region @@REGION@@ --query SecretString --output text > "$temp/secrets.json"
python3 - "$temp/fixture.json" "$temp/secrets.json" <<'PY'
import json,pathlib,pwd,sys
fixture=json.loads(pathlib.Path(sys.argv[1]).read_text())
secrets=json.loads(pathlib.Path(sys.argv[2]).read_text())
allowed={'OPENAI_API_KEY','DEEPGRAM_API_KEY','RING_APNS_KEY_BASE64','RING_APNS_KEY_ID','RING_APNS_TEAM_ID','RING_APNS_BUNDLE_ID','RING_FCM_SERVICE_ACCOUNT_BASE64'}
values={k:v for k,v in secrets.items() if k in allowed}
values.update(RING_ENV='test',RING_BIND='127.0.0.1:18767',RING_DATA_DIR='/var/lib/ring-devicecheck',RING_TEST_APP_SECRET=fixture['test_app_secret'],RING_TEST_TOKENS_FILE='/var/lib/ring-devicecheck/tokens.json',RING_DISABLE_PROVIDERS='0',RING_TEST_NATIVE_PUSH_ENABLED='true',RING_APNS_SANDBOX='true',RING_TELEMETRY_ENABLED='false')
with open('/etc/ring-devicecheck/server.env','w') as f:
    for k,v in values.items():
        assert isinstance(v,str) and '\x00' not in v
        f.write(k+'="'+v.replace('\\','\\\\').replace('"','\\"').replace('$','\\$').replace('`','\\`')+'"\n')
p=pathlib.Path('/var/lib/ring-devicecheck/tokens.json')
p.write_text(json.dumps(fixture['identities']))
p.chmod(0o600)
user=pwd.getpwnam('ring')
import os
os.chown(p,user.pw_uid,user.pw_gid)
PY
cat > /etc/systemd/system/ring-devicecheck.service <<'UNIT'
[Unit]
Description=Temporary isolated Ring physical-device check
After=network-online.target
[Service]
User=ring
Group=ring
WorkingDirectory=/var/lib/ring-devicecheck
EnvironmentFile=/etc/ring-devicecheck/server.env
ExecStart=/opt/ring/current/ring-server
Restart=on-failure
KillSignal=SIGINT
TimeoutStopSec=30
UMask=0077
NoNewPrivileges=true
ProtectSystem=strict
ProtectHome=true
PrivateTmp=true
ReadWritePaths=/var/lib/ring-devicecheck
UNIT
chmod 644 /etc/systemd/system/ring-devicecheck.service
systemctl daemon-reload
systemctl restart ring-devicecheck.service
healthy=0
for _ in $(seq 1 30); do
    if curl -fsS http://127.0.0.1:18767/health >/dev/null; then healthy=1; break; fi
    sleep 1
done
test "$healthy" = 1
python3 - <<'PY'
from pathlib import Path
p=Path('/etc/caddy/Caddyfile')
s=p.read_text()
if '# Temporary isolated physical-device endpoint' not in s:
    marker='backend.ring.teamofsilicons.com {'
    a,b=s.split(marker,1)
    old='    reverse_proxy 127.0.0.1:8765'
    assert b.count(old)==1
    b=b.replace(old,'    # Temporary isolated physical-device endpoint\n    handle_path /device-check/* {\n        reverse_proxy 127.0.0.1:18767\n    }\n    handle {\n        reverse_proxy 127.0.0.1:8765\n    }')
    p.write_text(a+marker+b)
PY
/usr/local/bin/caddy validate --config /etc/caddy/Caddyfile --adapter caddyfile
systemctl reload caddy.service
echo 'Isolated device-check service is ready.'
'''.replace("@@FIXTURE@@", q("s3://" + args.bucket + "/" + key)).replace("@@REGION@@", q(args.region)).replace("@@SECRET@@", q(args.secret_arn))
try:
    with tempfile.NamedTemporaryFile("w", prefix="ring-device-", suffix=".json") as request:
        json.dump({"commands": [script], "executionTimeout": ["180"]}, request)
        request.flush()
        command = aws("ssm", "send-command", "--instance-ids", args.instance_id, "--document-name", "AWS-RunShellScript", "--comment", "Isolated native physical-device check", "--parameters", "file://" + request.name)["Command"]["CommandId"]
    print(json.dumps({"command_id": command}), flush=True)
    for _ in range(70):
        time.sleep(3)
        try:
            result = aws("ssm", "get-command-invocation", "--command-id", command, "--instance-id", args.instance_id)
        except subprocess.CalledProcessError as error:
            if "InvocationDoesNotExist" in error.stderr:
                continue
            raise
        if result["Status"] in ("Pending", "InProgress", "Delayed"):
            continue
        print(result["StandardOutputContent"][-1500:])
        if result["Status"] != "Success":
            print(result["StandardErrorContent"][-2000:])
        raise SystemExit(0 if result["Status"] == "Success" else 1)
    raise TimeoutError("Inspect SSM command " + command)
finally:
    aws("s3api", "delete-object", "--bucket", args.bucket, "--key", key)
