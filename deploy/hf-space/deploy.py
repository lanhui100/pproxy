#!/usr/bin/env python3
"""Deploy pproxy-edge HF Space via CF Worker gateway (bypass GFW)."""
import base64
import json
import os
import sys
from pathlib import Path

import httpx

WORKER = "https://edge.ponyjob.top"
SECRET = os.environ["PROXY_SECRET"]
HF_TOKEN = os.environ["HF_TOKEN"]
REPO = "pproxy-edge"
SRC = Path("/home/USER/pproxy/deploy/hf-space")


def hf(method: str, path: str, **kw) -> httpx.Response:
    url = f"{WORKER}/x?url=https://huggingface.co{path}"
    r = httpx.request(
        method, url,
        headers={"X-Proxy-Secret": SECRET, "Authorization": f"Bearer {HF_TOKEN}", **kw.pop("headers", {})},
        timeout=60,
        **kw,
    )
    return r


def main():
    # 1. whoami
    r = hf("GET", "/api/whoami-v2")
    if r.status_code != 200:
        sys.exit(f"whoami failed: {r.status_code} {r.text[:200]}")
    user = r.json()["name"]
    repo_id = f"{user}/{REPO}"
    print(f"[1/4] user={user} repo={repo_id}")

    # 2. create space
    r = hf("POST", "/api/repos/create", json={
        "name": REPO, "organization": None, "type": "space",
        "sdk": "docker", "visibility": "public",
    })
    print(f"[2/4] create: {r.status_code} {r.text[:120]}")
    if r.status_code not in (200, 201, 409):
        sys.exit("create repo failed")

    # 3. set secret
    r = hf("POST", f"/api/spaces/{repo_id}/secrets", json={
        "key": "PROXY_SECRET", "value": SECRET,
    })
    print(f"[3/4] secret: {r.status_code}")
    if r.status_code not in (200, 201):
        sys.exit("set secret failed")

    # 4. upload files via NDJSON commit
    ops = [json.dumps({"key": "header", "value": {"summary": "deploy pproxy-edge", "description": ""}})]
    for f in ["README.md", "Dockerfile", "requirements.txt", "main.py"]:
        content = base64.b64encode((SRC / f).read_bytes()).decode()
        ops.append(json.dumps({"key": "file", "value": {"path": f, "content": content, "encoding": "base64"}}))
    ndjson = "\n".join(ops) + "\n"
    r = hf("POST", f"/api/spaces/{repo_id}/commit/main",
           headers={"Content-Type": "application/x-ndjson"}, content=ndjson.encode())
    print(f"[4/4] commit: {r.status_code} {r.text[:200]}")
    if r.status_code not in (200, 201):
        sys.exit("commit failed")

    print(f"\nDONE. Space URL: https://huggingface.co/spaces/{repo_id}")
    print(f"Runtime URL: https://{user}-{repo_id}.hf.space")


if __name__ == "__main__":
    main()
