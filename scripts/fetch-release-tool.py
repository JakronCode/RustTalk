#!/usr/bin/env python3
"""Download a versioned GitHub release tool and verify GitHub's asset digest."""
import hashlib
import json
import os
from pathlib import Path
import sys
import urllib.request

repo, tag, name, destination = sys.argv[1:]
headers = {"Accept": "application/vnd.github+json", "User-Agent": "RustTalk-CI"}
if os.environ.get("GH_TOKEN"):
    headers["Authorization"] = "Bearer " + os.environ["GH_TOKEN"]
request = urllib.request.Request(f"https://api.github.com/repos/{repo}/releases/tags/{tag}", headers=headers)
with urllib.request.urlopen(request, timeout=60) as response:
    release = json.load(response)
asset = next(a for a in release["assets"] if a["name"] == name)
digest = asset.get("digest") or ""
if not digest.startswith("sha256:"):
    raise SystemExit(f"No SHA-256 digest for {repo}/{tag}/{name}")
with urllib.request.urlopen(asset["browser_download_url"], timeout=120) as response:
    data = response.read()
if hashlib.sha256(data).hexdigest() != digest.removeprefix("sha256:"):
    raise SystemExit("Release tool checksum mismatch")
path = Path(destination)
path.write_bytes(data)
path.chmod(0o755)
