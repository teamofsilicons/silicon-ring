#!/usr/bin/env python3
"""Build a static Linux binary and web bundle on temporary AWS CodeBuild."""
import argparse
import json
from pathlib import Path
import re
import subprocess
import tempfile
import time
import uuid
import zipfile

parser = argparse.ArgumentParser(description=__doc__)
parser.add_argument("--region", default="us-west-1")
parser.add_argument("--revision", default="HEAD", help="Committed Git revision to build")
parser.add_argument("--bucket", required=True, help="Private destination bucket for the release")
args = parser.parse_args()
assert re.fullmatch(r"[a-z0-9.-]+", args.bucket)
assert re.fullmatch(r"[a-z0-9-]+", args.region)
receipt_path = Path("deploy/aws-build.private.json")
receipt = {"region": args.region, "release_bucket": args.bucket}


def save():
    receipt_path.write_text(json.dumps(receipt, indent=2) + "\n")
    receipt_path.chmod(0o600)


def aws(*command, payload=None):
    command = ["aws", *command, "--region", args.region, "--output", "json"]
    if payload is None:
        result = subprocess.run(command, capture_output=True, text=True)
    else:
        with tempfile.NamedTemporaryFile("w", prefix="ring-build-", suffix=".json") as request:
            json.dump(payload, request)
            request.flush()
            result = subprocess.run([*command, "--cli-input-json", "file://" + request.name], capture_output=True, text=True)
    if result.returncode:
        raise RuntimeError(result.stderr.strip())
    return json.loads(result.stdout) if result.stdout.strip() else {}


account = aws("sts", "get-caller-identity")["Account"]
suffix = uuid.uuid4().hex[:10]
name = "silicon-ring-builder-" + suffix
bucket = "silicon-ring-build-" + account + "-" + suffix
role_arn = f"arn:aws:iam::{account}:role/{name}"
project_arn = f"arn:aws:codebuild:{args.region}:{account}:project/{name}"
receipt.update({"project": name, "bucket": bucket, "role": name, "source_revision": subprocess.run(["git", "rev-parse", "--verify", args.revision + "^{commit}"], capture_output=True, text=True, check=True).stdout.strip()})
release_key = f"releases/{receipt['source_revision']}/ring-linux-amd64-{suffix}.tar.gz"
receipt["release_key"] = release_key
save()
created = set()
try:
    aws("s3api", "create-bucket", payload={"Bucket": bucket, "CreateBucketConfiguration": {"LocationConstraint": args.region}})
    created.add("bucket")
    aws("s3api", "put-public-access-block", payload={"Bucket": bucket, "PublicAccessBlockConfiguration": {"BlockPublicAcls": True, "IgnorePublicAcls": True, "BlockPublicPolicy": True, "RestrictPublicBuckets": True}})
    aws("s3api", "put-bucket-encryption", payload={"Bucket": bucket, "ServerSideEncryptionConfiguration": {"Rules": [{"ApplyServerSideEncryptionByDefault": {"SSEAlgorithm": "AES256"}}]}})
    with tempfile.NamedTemporaryFile(suffix=".zip", prefix="ring-source-") as source:
        inputs = ["Cargo.toml", "Cargo.lock", "scripts/prepare-web.mjs", "scripts/install.sh", "crates", "web"]
        subprocess.run(["git", "archive", "--format=zip", "--output", source.name, receipt["source_revision"], *inputs], check=True)
        with zipfile.ZipFile(source.name) as archive:
            for filename in archive.namelist():
                if Path(filename).name.startswith(".env") or filename.endswith((".private.json", ".p8", ".pem", ".key")):
                    raise RuntimeError("Protected material was found in committed build inputs")
        aws("s3api", "put-object", "--bucket", bucket, "--key", "source.zip", "--body", source.name, "--server-side-encryption", "AES256")
    trust = {"Version": "2012-10-17", "Statement": [{"Effect": "Allow", "Principal": {"Service": "codebuild.amazonaws.com"}, "Action": "sts:AssumeRole", "Condition": {"StringEquals": {"aws:SourceAccount": account}, "ArnEquals": {"aws:SourceArn": project_arn}}}]}
    aws("iam", "create-role", payload={"RoleName": name, "AssumeRolePolicyDocument": json.dumps(trust), "Description": "Temporary isolated Ring native binary builder"})
    created.add("role")
    policy = {"Version": "2012-10-17", "Statement": [
        {"Effect": "Allow", "Action": ["s3:GetObject", "s3:GetObjectVersion"], "Resource": f"arn:aws:s3:::{bucket}/source.zip"},
        {"Effect": "Allow", "Action": "s3:GetBucketLocation", "Resource": f"arn:aws:s3:::{bucket}"},
        {"Effect": "Allow", "Action": "s3:PutObject", "Resource": f"arn:aws:s3:::{args.bucket}/{release_key}"},
        {"Effect": "Allow", "Action": ["logs:CreateLogGroup", "logs:CreateLogStream", "logs:PutLogEvents"], "Resource": [f"arn:aws:logs:{args.region}:{account}:log-group:/aws/codebuild/{name}", f"arn:aws:logs:{args.region}:{account}:log-group:/aws/codebuild/{name}:*"]},
    ]}
    aws("iam", "put-role-policy", payload={"RoleName": name, "PolicyName": "ring-image-build", "PolicyDocument": json.dumps(policy)})
    buildspec = {"version": "0.2", "phases": {
        "install": {"commands": [
            "apt-get update -qq && apt-get install -y -qq musl-tools build-essential cmake pkg-config",
            "curl --fail --silent --show-error https://sh.rustup.rs -o /tmp/rustup-init.sh && sh /tmp/rustup-init.sh -y --profile minimal --default-toolchain 1.98.0 --target x86_64-unknown-linux-musl",
            "curl --fail --silent --show-error https://nodejs.org/dist/v24.21.0/node-v24.21.0-linux-x64.tar.xz -o /tmp/node-v24.21.0-linux-x64.tar.xz",
            "curl --fail --silent --show-error https://nodejs.org/dist/v24.21.0/SHASUMS256.txt -o /tmp/node-checksums && cd /tmp && grep ' node-v24.21.0-linux-x64.tar.xz$' node-checksums | sha256sum -c -",
            "mkdir -p /opt/ring-build-node && tar -xJf /tmp/node-v24.21.0-linux-x64.tar.xz -C /opt/ring-build-node --strip-components=1",
        ]},
        "build": {"commands": [
            "cd $CODEBUILD_SRC_DIR",
            "export PATH=/root/.cargo/bin:/opt/ring-build-node/bin:$PATH",
            "export CARGO_TARGET_X86_64_UNKNOWN_LINUX_MUSL_LINKER=musl-gcc CC_x86_64_unknown_linux_musl=musl-gcc",
            "export RUSTFLAGS='-C target-feature=+crt-static -C link-arg=-static'",
            "cargo +1.98.0 build --locked --release --target x86_64-unknown-linux-musl -p ring-server --features ring-providers/s3",
            "npm --prefix web ci && npm --prefix web run build",
            "mkdir /tmp/ring-release && cp target/x86_64-unknown-linux-musl/release/ring-server /tmp/ring-release/ring-server && cp -a web/dist /tmp/ring-release/web",
            "file /tmp/ring-release/ring-server",
            "if readelf -l /tmp/ring-release/ring-server | grep -q INTERP; then echo 'Expected a static binary without an ELF interpreter' >&2; exit 1; fi",
            "tar -C /tmp/ring-release -czf /tmp/ring-linux-amd64.tar.gz ring-server web",
            f"checksum=$(sha256sum /tmp/ring-linux-amd64.tar.gz | cut -d' ' -f1) && aws s3api put-object --region {args.region} --bucket {args.bucket} --key {release_key} --body /tmp/ring-linux-amd64.tar.gz --server-side-encryption AES256 --metadata sha256=$checksum,revision={receipt['source_revision']}",
        ]},
    }}
    project = {"name": name, "source": {"type": "S3", "location": bucket + "/source.zip", "buildspec": json.dumps(buildspec)}, "artifacts": {"type": "NO_ARTIFACTS"}, "environment": {"type": "LINUX_CONTAINER", "image": "aws/codebuild/standard:7.0", "computeType": "BUILD_GENERAL1_LARGE", "privilegedMode": False}, "serviceRole": role_arn, "timeoutInMinutes": 45, "queuedTimeoutInMinutes": 30}
    for attempt in range(6):
        try:
            aws("codebuild", "create-project", payload=project)
            break
        except RuntimeError as error:
            if "assume" not in str(error).lower() or attempt == 5:
                raise
            time.sleep(3)
    created.add("project")
    build_id = aws("codebuild", "start-build", "--project-name", name)["build"]["id"]
    receipt["build_id"] = build_id
    save()
    print(json.dumps({"build_id": build_id, "release_key": release_key}), flush=True)
    previous_phase = None
    while True:
        time.sleep(8)
        build = aws("codebuild", "batch-get-builds", "--ids", build_id)["builds"][0]
        if build["currentPhase"] != previous_phase:
            previous_phase = build["currentPhase"]
            print(json.dumps({"phase": previous_phase, "status": build["buildStatus"]}), flush=True)
        if build["buildStatus"] == "IN_PROGRESS":
            continue
        receipt["status"] = build["buildStatus"]
        if build.get("logs", {}).get("groupName") and build["logs"].get("streamName"):
            logs = aws("logs", "get-log-events", "--log-group-name", build["logs"]["groupName"], "--log-stream-name", build["logs"]["streamName"], "--limit", "100")
            Path("/tmp/ring-aws-build.log").write_text("\n".join(x["message"] for x in logs["events"]))
        save()
        if build["buildStatus"] != "SUCCEEDED":
            print(json.dumps({"phases": build["phases"]}), flush=True)
            raise RuntimeError("Remote native build failed; inspect /tmp/ring-aws-build.log")
        receipt["release_sha256"] = aws("s3api", "head-object", "--bucket", args.bucket, "--key", release_key)["Metadata"]["sha256"]
        assert re.fullmatch(r"[a-f0-9]{64}", receipt["release_sha256"])
        save()
        setup_path = Path("deploy/aws-setup.private.json")
        setup = json.loads(setup_path.read_text())
        setup.update({"native_bundle_key": release_key, "native_bundle_sha256": receipt["release_sha256"], "native_bundle_revision": receipt["source_revision"], "release_bucket": args.bucket})
        setup_path.write_text(json.dumps(setup, indent=2) + "\n")
        print(json.dumps({"release_key": release_key, "sha256": receipt["release_sha256"], "status": "SUCCEEDED"}), flush=True)
        break
finally:
    errors = []
    if receipt.get("build_id") and not receipt.get("status"):
        try:
            aws("codebuild", "stop-build", "--id", receipt["build_id"])
        except Exception as error:
            errors.append(str(error))
    for resource, command in [
        ("project", ["codebuild", "delete-project", "--name", name]),
        ("role", ["iam", "delete-role-policy", "--role-name", name, "--policy-name", "ring-image-build"]),
        ("role", ["iam", "delete-role", "--role-name", name]),
        ("bucket", ["s3api", "delete-object", "--bucket", bucket, "--key", "source.zip"]),
        ("bucket", ["s3api", "delete-bucket", "--bucket", bucket]),
    ]:
        if resource in created:
            try:
                aws(*command)
            except Exception as error:
                errors.append(str(error))
    if "project" in created:
        try:
            aws("logs", "delete-log-group", "--log-group-name", "/aws/codebuild/" + name)
        except Exception as error:
            if "ResourceNotFoundException" not in str(error):
                errors.append(str(error))
    receipt["cleanup"] = "complete" if not errors else errors
    save()
    print(json.dumps({"builder_cleanup": receipt["cleanup"]}), flush=True)
