#!/usr/bin/env python3
"""Replace Ring's deployed container through SSM, rolling back if readiness fails."""
import argparse
import json
import re
import shlex
import subprocess
import tempfile
import time
import uuid

parser = argparse.ArgumentParser(description=__doc__)
parser.add_argument("--instance-id", required=True)
parser.add_argument("--image", required=True)
parser.add_argument("--secret-arn", required=True)
parser.add_argument("--bucket", required=True)
parser.add_argument("--region", default="us-west-1")
args = parser.parse_args()
assert re.fullmatch(r"i-[0-9a-f]+", args.instance_id)
assert re.fullmatch(r"[0-9]+\.dkr\.ecr\.[a-z0-9-]+\.amazonaws\.com/silicon-ring@sha256:[0-9a-f]{64}", args.image)
assert re.fullmatch(r"arn:aws:secretsmanager:[a-z0-9-]+:[0-9]+:secret:[A-Za-z0-9/_+=.@-]+", args.secret_arn)
assert re.fullmatch(r"[a-z0-9.-]+", args.bucket)
q = shlex.quote
previous = "ring-previous-" + uuid.uuid4().hex[:12]
script = f"""#!/bin/bash
set -Eeuo pipefail
umask 077
cloud-init status --wait >/dev/null
aws ecr get-login-password --region {q(args.region)} | docker login --username AWS --password-stdin {q(args.image.split('/')[0])} >/dev/null
docker pull {q(args.image)} >/dev/null
previous={q(previous)}
previous_env=/etc/ring/{q(previous)}.env
cp /etc/ring/server.env "$previous_env"
rollback() {{
  code=$?
  if docker inspect "$previous" >/dev/null 2>&1; then
    docker rm -f ring >/dev/null 2>&1 || true
    docker rename "$previous" ring
  fi
  mv "$previous_env" /etc/ring/server.env
  docker start ring >/dev/null 2>&1 || true
  echo 'Ring update failed; previous container restored.' >&2
  exit "$code"
}}
trap rollback ERR
aws secretsmanager get-secret-value --region {q(args.region)} --secret-id {q(args.secret_arn)} --query SecretString --output text | jq -er 'to_entries | map(select(.value | type == "string") | "\\(.key)=\\(.value)") | join("\\n")' > /etc/ring/server.env
printf '\\nRING_ENV=production\\nRING_S3_BUCKET=%s\\nAWS_REGION=%s\\n' {q(args.bucket)} {q(args.region)} >> /etc/ring/server.env
docker stop --time 15 ring >/dev/null
docker rename ring "$previous"
docker run -d --name ring --restart unless-stopped --network ring --security-opt no-new-privileges:true --env-file /etc/ring/server.env --env RING_BIND=0.0.0.0:8765 --volume /var/lib/ring:/data {q(args.image)} >/dev/null
healthy=0
for n in $(seq 1 60); do
  if docker exec ring curl --fail --silent http://127.0.0.1:8765/health >/dev/null; then healthy=1; break; fi
  sleep 1
done
test "$healthy" = 1
trap - ERR
docker rm "$previous" >/dev/null
rm "$previous_env"
echo 'Ring image updated and readiness passed.'
"""


def aws(*command):
    result = subprocess.run(["aws", *command, "--region", args.region, "--output", "json"], capture_output=True, text=True, check=True)
    return json.loads(result.stdout)


with tempfile.NamedTemporaryFile("w", prefix="ring-update-", suffix=".json") as parameters:
    json.dump({"commands": [script], "executionTimeout": ["900"]}, parameters)
    parameters.flush()
    command_id = aws("ssm", "send-command", "--instance-ids", args.instance_id, "--document-name", "AWS-RunShellScript", "--comment", "Update Ring image with readiness rollback", "--parameters", "file://" + parameters.name)["Command"]["CommandId"]
print(json.dumps({"command_id": command_id, "instance_id": args.instance_id}), flush=True)
deadline = time.monotonic() + 960
while time.monotonic() < deadline:
    time.sleep(3)
    try:
        result = aws("ssm", "get-command-invocation", "--command-id", command_id, "--instance-id", args.instance_id)
    except subprocess.CalledProcessError as error:
        if "InvocationDoesNotExist" in error.stderr:
            continue
        raise
    if result["Status"] in ("Pending", "InProgress", "Delayed"):
        continue
    print(result["StandardOutputContent"].strip())
    print(json.dumps({"status": result["Status"]}))
    if result["Status"] != "Success":
        print(result["StandardErrorContent"][-2000:])
    raise SystemExit(0 if result["Status"] == "Success" else 1)
raise TimeoutError("Inspect SSM command " + command_id)
