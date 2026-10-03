#!/usr/bin/env python3
"""Register Ring's notification types after Honeycomb recognizes its IAM identity."""
import json
from pathlib import Path
import subprocess


for item in json.loads(Path(__file__).with_name("ting-types.json").read_text()):
    subprocess.run(
        ["ting", "types", "register", "--org", "tos", "--type", item["type"],
         "--description", item["description"], "--json"],
        check=True,
    )
