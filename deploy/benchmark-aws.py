#!/usr/bin/env python3
"""Run the real media benchmark in a separate container on the deployed EC2 host."""
import argparse
import base64
import datetime
import json
from pathlib import Path
import re
import shlex
import subprocess
import tempfile
import time
import uuid


def aws(*args):
    result = subprocess.run(["aws", *args, "--region", options.region, "--output", "json"], capture_output=True, text=True)
    if result.returncode:
        raise RuntimeError(result.stderr.strip())
    return json.loads(result.stdout)


parser = argparse.ArgumentParser(description=__doc__)
parser.add_argument("--instance-id", required=True)
parser.add_argument("--image", required=True, help="Immutable ECR image URI")
parser.add_argument("--bucket", required=True, help="Ring private assets bucket for the result")
parser.add_argument("--region", default="us-west-1")
parser.add_argument("--report", type=Path, default=Path("docs/benchmarks/2026-10-03-aws.json"))
options = parser.parse_args()
assert re.fullmatch(r"i-[0-9a-f]+", options.instance_id)
assert re.fullmatch(r"[a-z0-9.-]+", options.bucket)
assert re.fullmatch(r"[0-9]+\.dkr\.ecr\.[a-z0-9-]+\.amazonaws\.com/silicon-ring@sha256:[0-9a-f]{64}", options.image)
run = "ring-load-" + datetime.datetime.now(datetime.timezone.utc).strftime("%Y%m%dT%H%M%SZ") + "-" + uuid.uuid4().hex[:8]
object_key = f"benchmarks/{run}.json"
source = base64.b64encode(Path("scripts/load-test.mjs").read_bytes()).decode()
q = shlex.quote
script = f"""#!/bin/bash
set -eu
umask 077
directory=$(mktemp -d /var/tmp/ring-load.XXXXXX)
name={q(run)}
cleanup() {{
  docker rm -f "$name" >/dev/null 2>&1 || true
  python3 - "$directory" <<'PY'
import pathlib,shutil,sys
p=pathlib.Path(sys.argv[1]).resolve()
assert p.parent==pathlib.Path('/var/tmp') and p.name.startswith('ring-load.')
shutil.rmtree(p)
PY
}}
trap cleanup EXIT
printf '%s' '{source}' | base64 -d > "$directory/load-test.mjs"
docker run --rm --network host -v "$directory:/benchmark" -w /benchmark node:24-bookworm-slim node load-test.mjs --prepare-fixtures /benchmark/fixture.private.json
mkdir "$directory/data"
chown 10001:10001 "$directory/data" "$directory/fixture.private.json.tokens.private.json"
printf 'RING_ENV=test\nRING_BIND=0.0.0.0:8765\nRING_DISABLE_PROVIDERS=1\nRING_TELEMETRY_ENABLED=false\nRING_TEST_TOKENS_FILE=/tokens.json\n' > "$directory/environment"
printf 'RING_TEST_APP_SECRET=%s\n' "$(jq -r .test_app_secret "$directory/fixture.private.json")" >> "$directory/environment"
docker run -d --name "$name" --security-opt no-new-privileges:true --env-file "$directory/environment" -p 127.0.0.1:18766:8765 -v "$directory/data:/data" -v "$directory/fixture.private.json.tokens.private.json:/tokens.json:ro" {q(options.image)} >/dev/null
healthy=0
for n in $(seq 1 60); do
  if curl -fsS http://127.0.0.1:18766/health >/dev/null; then healthy=1; break; fi
  sleep 1
done
test "$healthy" = 1
pid=$(docker inspect --format '{{{{.State.Pid}}}}' "$name")
result=0
docker run --rm --network host --pid host -v "$directory:/benchmark" -w /benchmark node:24-bookworm-slim node load-test.mjs --url ws://127.0.0.1:18766/ws --secrets-file /benchmark/fixture.private.json --pid "$pid" --calls 15 --seconds 30 --report /benchmark/report.json > "$directory/output.log" || result=$?
test -s "$directory/report.json"
aws s3 cp "$directory/report.json" {q('s3://' + options.bucket + '/' + object_key)} --region {q(options.region)} --sse AES256 --only-show-errors
jq '{{passed,calls,elapsed_seconds,pcm,relay_latency_ms,sender_lateness_ms,server_resources,errors}}' "$directory/report.json"
exit "$result"
"""
with tempfile.NamedTemporaryFile("w", prefix="ring-ssm-", suffix=".json") as parameters:
    json.dump({"commands": [script], "executionTimeout": ["900"]}, parameters)
    parameters.flush()
    command = aws("ssm", "send-command", "--instance-ids", options.instance_id, "--document-name", "AWS-RunShellScript", "--comment", "Isolated Ring 15-call benchmark", "--parameters", "file://" + parameters.name)["Command"]["CommandId"]
print(json.dumps({"command_id": command, "instance_id": options.instance_id, "report_key": object_key}), flush=True)
deadline = time.monotonic() + 960
while time.monotonic() < deadline:
    time.sleep(3)
    try:
        invocation = aws("ssm", "get-command-invocation", "--command-id", command, "--instance-id", options.instance_id)
    except RuntimeError as error:
        if "InvocationDoesNotExist" in str(error):
            continue
        raise
    if invocation["Status"] in ("Pending", "InProgress", "Delayed"):
        continue
    options.report.parent.mkdir(parents=True, exist_ok=True)
    result = subprocess.run(["aws", "s3", "cp", f"s3://{options.bucket}/{object_key}", str(options.report), "--region", options.region, "--only-show-errors"], capture_output=True, text=True)
    if result.returncode:
        print(invocation.get("StandardErrorContent", "")[-3000:])
        raise RuntimeError("AWS benchmark did not produce a report: " + invocation["Status"])
    report = json.loads(options.report.read_text())
    instance = aws("ec2", "describe-instances", "--instance-ids", options.instance_id)["Reservations"][0]["Instances"][0]
    report["aws"] = {"instance_id": options.instance_id, "instance_type": instance["InstanceType"], "region": options.region, "image": options.image, "ssm_command_id": command}
    report["scope"] = "Isolated PCM relay and recording benchmark on the deployed host; external provider network calls disabled"
    options.report.write_text(json.dumps(report, indent=2) + "\n")
    print(json.dumps({"passed": report["passed"], "report": str(options.report), "relay_latency_ms": report["relay_latency_ms"], "server_resources": report["server_resources"], "errors": report["errors"][:5]}, indent=2))
    raise SystemExit(0 if report["passed"] and invocation["Status"] == "Success" else 1)
raise TimeoutError("AWS benchmark command did not finish within 16 minutes; inspect " + command)
