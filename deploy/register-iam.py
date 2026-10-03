#!/usr/bin/env python3
"""Register exactly the reviewed deploy/iam-app.json. Never echo generated credentials."""
import json
import os
from pathlib import Path
import secrets
import subprocess
import sys

path = Path(__file__).with_name("iam-app.json")
config = json.loads(path.read_text())
output = Path(sys.argv[1] if len(sys.argv) > 1 else "deploy/iam-registration.private.json")
if output.exists():
    sys.exit("Refusing to overwrite existing IAM registration. Inspect it and the IAM app before retrying.")
webhook_secret = secrets.token_urlsafe(48)
args = ["iam", "app", "create", config["app_id"], "--org", config["org_id"], "--name", config["name"],
        "--webhook-url", config["webhook_url"], "--webhook-secret", webhook_secret, "--base-url", config["base_url"],
        "--app-scope", json.dumps(config["app_scope"]), "--webhook-scope", ",".join(config["webhook_scope"]), "--json"]
# Persist the chosen webhook secret before the mutation, protecting an uncertain response.
fd = os.open(output, os.O_WRONLY | os.O_CREAT | os.O_EXCL, 0o600)
with os.fdopen(fd, "w") as saved:
    json.dump({"state": "registration_pending", "webhook_secret": webhook_secret, "app_id": config["app_id"]}, saved)
    saved.flush()
    os.fsync(saved.fileno())
result = subprocess.run(args, capture_output=True, text=True)
try:
    response = json.loads(result.stdout)
except json.JSONDecodeError:
    response = {"unparsed_response": result.stdout, "stderr": result.stderr}
with output.open("w") as saved:
    json.dump({"state": "registered" if result.returncode == 0 else "needs_review", "webhook_secret": webhook_secret, "response": response}, saved)
    saved.flush()
    os.fsync(saved.fileno())
print(f"IAM result saved with mode 0600 to {output}; exit status {result.returncode}.")
if result.returncode:
    print("Review the saved response privately. Do not rerun registration blindly after an uncertain result.")
sys.exit(result.returncode)
