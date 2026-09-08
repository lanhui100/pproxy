#!/usr/bin/env python3
"""用 CLI 自己的 OAuth 令牌直调 Cloud Code 后端，打印**错误类别**而非 UI 文案。

用途：把 "400 location" / "503 容量" / "403 许可" / "401" 区分开——这是判断
"问题在代理层还是 Google 层" 的最快手段（代理层不参与错误码选择）。

用法:
    python3 cloudcode_probe.py [--proxy http://127.0.0.1:8899] [--host daily-cloudcode-pa.googleapis.com]

令牌来源: ~/.gemini/antigravity-cli/antigravity-oauth-token（CLI 登录态）
"""
from __future__ import annotations

import argparse
import json
import os
import urllib.error
import urllib.request

TOKEN_FILE = "~/.gemini/antigravity-cli/antigravity-oauth-token"
PROJECT = "default-cli-project"


def load_token() -> str:
    path = os.path.expanduser(TOKEN_FILE)
    data = json.load(open(path, encoding="utf-8"))
    tok = (data.get("token") or {}).get("access_token", "")
    if not tok:
        raise SystemExit(f"未在 {path} 找到 access_token")
    return tok


def call(url: str, token: str, proxy: str | None, body: dict | None = None) -> None:
    data = json.dumps(body).encode() if body is not None else None
    req = urllib.request.Request(
        url,
        data=data,
        headers={
            "Authorization": f"Bearer {token}",
            "Content-Type": "application/json",
            "User-Agent": "pproxy-diag/1.0",
        },
        method="POST" if data else "GET",
    )
    opener = (
        urllib.request.build_opener(urllib.request.ProxyHandler({"http": proxy, "https": proxy}))
        if proxy
        else urllib.request.build_opener(urllib.request.ProxyHandler({}))
    )
    name = url.rsplit("/", 1)[-1]
    try:
        with opener.open(req, timeout=30) as resp:
            body_text = resp.read(600).decode("utf-8", "replace")
            print(f"[{resp.status}] {name}\n    {body_text[:400]}")
    except urllib.error.HTTPError as e:
        payload = e.read(800).decode("utf-8", "replace")
        reason = ""
        try:
            err = json.loads(payload).get("error", {})
            reason = f" reason={err.get('status')}"
            for d in err.get("details", []):
                if d.get("reason"):
                    reason += f" {d['reason']}"
                    md = d.get("metadata") or {}
                    if md.get("error_number"):
                        reason += f" error_number={md['error_number']}"
        except Exception:
            pass
        print(f"[{e.code}] {name}{reason}\n    {payload[:400]}")
    except Exception as e:  # 网络/代理类
        print(f"[ERR] {name} {type(e).__name__}: {e}")


def main() -> int:
    ap = argparse.ArgumentParser()
    ap.add_argument("--proxy", default=os.environ.get("https_proxy") or "http://127.0.0.1:8899")
    ap.add_argument("--host", default="daily-cloudcode-pa.googleapis.com")
    args = ap.parse_args()

    token = load_token()
    base = f"https://{args.host}/v1internal:"
    print(f"proxy={args.proxy}  host={args.host}\n")
    call(base + "loadCodeAssist", token, args.proxy, {"cloudaicompanionProject": PROJECT})
    call(base + "fetchAvailableModels", token, args.proxy, {"project": PROJECT})
    call(
        base + "streamGenerateContent?alt=sse",
        token,
        args.proxy,
        {
            "model": "gemini-3.1-pro",
            "project": PROJECT,
            "request": {"contents": [{"role": "user", "parts": [{"text": "say OK"}]}]},
        },
    )
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
