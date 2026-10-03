#!/usr/bin/env python3
"""Benchmark the deployed native binary in a separate, isolated systemd process."""
import argparse
import datetime
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
parser.add_argument("--mode", choices=["relay", "live"], default="relay")
parser.add_argument("--secret-arn", help="Required only for the bounded paid Live provider check")
parser.add_argument("--region", default="us-west-1")
parser.add_argument("--calls", type=int, default=15)
parser.add_argument("--seconds", type=int, default=30)
parser.add_argument("--report", type=Path, required=True)
options = parser.parse_args()
assert re.fullmatch(r"i-[0-9a-f]+", options.instance_id)
assert re.fullmatch(r"[a-z0-9.-]+", options.bucket)
assert 1 <= options.calls <= 15 and 10 <= options.seconds <= 120
assert options.mode != "live" or options.secret_arn, "Live mode needs --secret-arn"

def aws(*command):
    result = subprocess.run(["aws", *command, "--region", options.region, "--output", "json"], capture_output=True, text=True)
    if result.returncode:
        raise RuntimeError(result.stderr.strip())
    return json.loads(result.stdout) if result.stdout.strip() else {}

run = "ring-load-" + datetime.datetime.now(datetime.timezone.utc).strftime("%Y%m%dT%H%M%SZ") + "-" + uuid.uuid4().hex[:8]
object_key = "benchmarks/" + run + ".json"
script_key = "benchmarks/" + run + ".mjs"
source = Path("scripts/load-live.mjs" if options.mode == "live" else "scripts/load-test.mjs")
aws("s3api", "put-object", "--bucket", options.bucket, "--key", script_key, "--body", str(source), "--server-side-encryption", "AES256")
q = shlex.quote
script = r'''#!/bin/bash
set -euo pipefail
umask 077
directory=$(mktemp -d /var/tmp/ring-load.XXXXXXXX)
name=@@NAME@@
cleanup() {
  systemctl stop "$name.service" >/dev/null 2>&1 || true
  systemctl reset-failed "$name.service" >/dev/null 2>&1 || true
  python3 - "$directory" <<'PYREMOVE'
import pathlib,shutil,sys
p=pathlib.Path(sys.argv[1]).resolve()
assert p.parent==pathlib.Path('/var/tmp') and p.name.startswith('ring-load.')
shutil.rmtree(p)
PYREMOVE
}
trap cleanup EXIT
if test ! -x /opt/ring-benchmark-node/bin/node; then
  curl -fsS https://nodejs.org/dist/v24.21.0/node-v24.21.0-linux-x64.tar.xz -o "$directory/node-v24.21.0-linux-x64.tar.xz"
  curl -fsS https://nodejs.org/dist/v24.21.0/SHASUMS256.txt -o "$directory/checksums"
  (cd "$directory" && grep ' node-v24.21.0-linux-x64.tar.xz$' checksums | sha256sum -c -)
  install -d -m 755 /opt/ring-benchmark-node
  tar -xJf "$directory/node-v24.21.0-linux-x64.tar.xz" -C /opt/ring-benchmark-node --strip-components=1
fi
node=/opt/ring-benchmark-node/bin/node
aws s3 cp @@SOURCE@@ "$directory/check.mjs" --region @@REGION@@ --only-show-errors
"$node" "$directory/check.mjs" --calls @@CALLS@@ --prepare-fixtures "$directory/fixture.private.json"
if test @@MODE@@ = live; then
  aws secretsmanager get-secret-value --secret-id @@SECRET@@ --region @@REGION@@ --query SecretString --output text > "$directory/provider-secrets.json"
fi
mkdir "$directory/data"
python3 - "$directory" @@MODE@@ <<'PYENV'
import json,pathlib,pwd,os,sys
root=pathlib.Path(sys.argv[1])
fixture=json.loads((root/'fixture.private.json').read_text())
values={}
if sys.argv[2]=='live':
    p=root/'provider-secrets.json'
    secrets=json.loads(p.read_text())
    values={k:secrets[k] for k in ['OPENAI_API_KEY','DEEPGRAM_API_KEY']}
    p.unlink()
values.update(RING_ENV='test',RING_BIND='127.0.0.1:18766',RING_DATA_DIR=str(root/'data'),RING_TEST_APP_SECRET=fixture['test_app_secret'],RING_TEST_TOKENS_FILE=str(root/'fixture.private.json.tokens.private.json'),RING_DISABLE_PROVIDERS='0' if sys.argv[2]=='live' else '1',RING_TELEMETRY_ENABLED='false')
with (root/'environment').open('w') as f:
    for key,value in values.items():
        assert isinstance(value,str) and '\x00' not in value
        value=value.replace('\\','\\\\').replace('"','\\"').replace('$','\\$').replace('`','\\`')
        f.write(key+'="'+value+'"\n')
user=pwd.getpwnam('ring')
for p in [root,root/'data',root/'fixture.private.json.tokens.private.json']:
    os.chown(p,user.pw_uid,user.pw_gid)
PYENV
binary=$(readlink -f /opt/ring/current/ring-server)
systemd-run --quiet --unit "$name" --property User=ring --property Group=ring --property "WorkingDirectory=$directory/data" --property "EnvironmentFile=$directory/environment" --property NoNewPrivileges=true --property ProtectSystem=strict --property "ReadWritePaths=$directory" --property KillSignal=SIGINT --property TimeoutStopSec=30 "$binary"
healthy=0
for _ in $(seq 1 60); do
  if curl -fsS http://127.0.0.1:18766/health >/dev/null; then healthy=1; break; fi
  sleep 1
done
test "$healthy" = 1
pid=$(systemctl show "$name.service" --property MainPID --value)
result=0
"$node" "$directory/check.mjs" --url ws://127.0.0.1:18766/ws --secrets-file "$directory/fixture.private.json" --pid "$pid" --calls @@CALLS@@ --seconds @@SECONDS@@ --report "$directory/report.json" @@PAID@@ > "$directory/output.log" 2>&1 || result=$?
if test ! -s "$directory/report.json"; then tail -n 40 "$directory/output.log"; exit 1; fi
python3 - "$directory/report.json" "$binary" <<'PYRELEASE'
import json,pathlib,sys
p=pathlib.Path(sys.argv[1])
v=json.loads(p.read_text())
v['deployment']={'release_sha256':pathlib.Path(sys.argv[2]).parent.name}
p.write_text(json.dumps(v,indent=2)+'\n')
PYRELEASE
aws s3 cp "$directory/report.json" @@REPORT@@ --region @@REGION@@ --sse AES256 --only-show-errors
python3 - "$directory/report.json" <<'PYSUMMARY'
import json,sys
v=json.load(open(sys.argv[1]))
print(json.dumps({k:v.get(k) for k in ['passed','calls','elapsed_seconds','relay_latency_ms','server_resources','errors']}))
PYSUMMARY
exit "$result"
'''
for key,value in {"NAME":run,"SOURCE":"s3://"+options.bucket+"/"+script_key,"REGION":options.region,"CALLS":str(options.calls),"SECONDS":str(options.seconds),"MODE":options.mode,"SECRET":options.secret_arn or "unused","REPORT":"s3://"+options.bucket+"/"+object_key}.items():
    script=script.replace("@@"+key+"@@",q(value))
script=script.replace("@@PAID@@","--paid-provider-smoke" if options.mode=="live" else "")
try:
    with tempfile.NamedTemporaryFile("w", prefix="ring-ssm-", suffix=".json") as parameters:
        json.dump({"commands":[script],"executionTimeout":["900"]},parameters)
        parameters.flush()
        command=aws("ssm","send-command","--instance-ids",options.instance_id,"--document-name","AWS-RunShellScript","--comment","Isolated native Ring " + options.mode + " benchmark","--parameters","file://"+parameters.name)["Command"]["CommandId"]
    print(json.dumps({"command_id":command,"instance_id":options.instance_id,"report_key":object_key}),flush=True)
    deadline=time.monotonic()+960
    while time.monotonic()<deadline:
        time.sleep(3)
        try:
            invocation=aws("ssm","get-command-invocation","--command-id",command,"--instance-id",options.instance_id)
        except RuntimeError as error:
            if "InvocationDoesNotExist" in str(error): continue
            raise
        if invocation["Status"] in ("Pending","InProgress","Delayed"): continue
        options.report.parent.mkdir(parents=True,exist_ok=True)
        result=subprocess.run(["aws","s3","cp","s3://"+options.bucket+"/"+object_key,str(options.report),"--region",options.region,"--only-show-errors"],capture_output=True,text=True)
        if result.returncode:
            print(invocation.get("StandardOutputContent", "")[-3000:])
            print(invocation.get("StandardErrorContent", "")[-3000:])
            raise RuntimeError("AWS benchmark did not produce a report: "+invocation["Status"])
        report=json.loads(options.report.read_text())
        instance=aws("ec2","describe-instances","--instance-ids",options.instance_id)["Reservations"][0]["Instances"][0]
        report["aws"]={"instance_id":options.instance_id,"instance_type":instance["InstanceType"],"region":options.region,"ssm_command_id":command,"runtime":"native systemd","mode":options.mode}
        options.report.write_text(json.dumps(report,indent=2)+"\n")
        print(json.dumps({"passed":report["passed"],"report":str(options.report),"relay_latency_ms":report.get("relay_latency_ms"),"server_resources":report["server_resources"],"errors":report["errors"][:5]},indent=2))
        raise SystemExit(0 if report["passed"] and invocation["Status"]=="Success" else 1)
    raise TimeoutError("Inspect SSM command "+command)
finally:
    aws("s3api","delete-object","--bucket",options.bucket,"--key",script_key)
