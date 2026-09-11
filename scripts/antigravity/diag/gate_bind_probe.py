#!/usr/bin/env python3
"""探测 gate 对指定 host 的 bind 判定（含出口地理门禁的拒绝原因）。

用途：验证 CF gate 的合规出口门禁是否生效——
    {"ok":true}                            → 放行
    {"ok":false,"reason":"unsupported_egress:US"}  → 出站 IP 国家不在白名单（fail-closed）
    {"ok":false,"reason":"unsupported_colo:HKG"}   → 入站 colo 被门禁拦截
    {"ok":false,"reason":"acl denied"}             → host/端口不合法

用法:
    python3 gate_bind_probe.py [ws-url] [host ...]
默认: wss://gate.example.com/ws  与 §手册里的典型 host
token: 环境变量 PPROXY_TUNNEL_TOKEN，或 ~/.pony/config.toml 的 tunnel_token
依赖: pip install websockets
"""
from __future__ import annotations

import asyncio
import json
import os
import re
import sys

try:
    import websockets
except ImportError:
    raise SystemExit("需要 websockets：pip install websockets")

DEFAULT_URL = "wss://gate.example.com/ws"
DEFAULT_HOSTS = [
    "github.com",                     # 非 Google：不受门禁
    "oauth2.googleapis.com",          # 泛 Google：只受 colo 门禁
    "daily-cloudcode-pa.googleapis.com",  # 合规出口 host：额外受出站地理门禁
    "www.google.com",
]


def load_token() -> str:
    tok = os.environ.get("PPROXY_TUNNEL_TOKEN")
    if tok:
        return tok.strip()
    cfg = os.path.expanduser("~/.pony/config.toml")
    try:
        text = open(cfg, encoding="utf-8").read()
    except OSError:
        raise SystemExit(f"未找到 {cfg}，且未设置 PPROXY_TUNNEL_TOKEN")
    m = re.search(r'^\s*tunnel_token\s*=\s*"?([^"\n]+)"?', text, re.M)
    if not m:
        raise SystemExit(f"未在 {cfg} 找到 tunnel_token")
    return m.group(1).strip()


async def probe(url: str, host: str, token: str) -> str:
    try:
        async with websockets.connect(
            url, additional_headers={"Authorization": f"Bearer {token}"}, open_timeout=20
        ) as ws:
            await ws.send(json.dumps({"host": host, "port": 443}))
            msg = await asyncio.wait_for(ws.recv(), timeout=20)
            return "BINARY(隧道已建立)" if isinstance(msg, bytes) else msg
    except Exception as e:  # noqa: BLE001
        return f"ERR {type(e).__name__}: {e}"


async def main() -> int:
    url = sys.argv[1] if len(sys.argv) > 1 else DEFAULT_URL
    hosts = sys.argv[2:] or DEFAULT_HOSTS
    token = load_token()
    print(f"gate={url}\n")
    for host in hosts:
        print(f"  {host:38} -> {await probe(url, host, token)}")
    return 0


if __name__ == "__main__":
    raise SystemExit(asyncio.run(main()))
