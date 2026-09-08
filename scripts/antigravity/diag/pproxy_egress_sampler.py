#!/usr/bin/env python3
"""采样 pproxy 到各 gate 出口的字节增量，判定"某次请求实际走了哪条出口"。

为什么需要它：`ss -tn` 只能看到连接存在（池化会同时预建 CF 与 Vercel 两条待命会话），
**看到连接 ≠ 用了那条连接**。只有字节计数器的增量能证明流量落在哪条出口上。

用法:
    python3 pproxy_egress_sampler.py [持续秒数] [pproxy-server PID]
PID 缺省时自动从 systemd 用户服务读取。

输出: 每个对端 (IP:port) 的 sent/recv 增量；触发一次失败请求后对比时间窗即可。
"""
from __future__ import annotations

import collections
import re
import subprocess
import sys
import time

LOCAL_PREFIXES = ("127.", "10.", "172.1", "192.168.")


def resolve_pid(explicit: str | None) -> str:
    if explicit:
        return explicit
    try:
        out = subprocess.run(
            ["systemctl", "--user", "show", "pproxy-server", "-p", "MainPID", "--value"],
            capture_output=True, text=True, timeout=5,
        ).stdout.strip()
        if out and out != "0":
            return out
    except Exception:
        pass
    raise SystemExit("无法确定 pproxy-server PID，请作为第二个参数传入")


def snapshot(pid: str) -> dict[str, tuple[int, int]]:
    try:
        out = subprocess.run(["ss", "-tinp"], capture_output=True, text=True, timeout=5).stdout
    except Exception:
        return {}
    lines = out.split("\n")
    result: dict[str, tuple[int, int]] = {}
    for i, line in enumerate(lines):
        if f"pid={pid}," not in line or "ESTAB" not in line:
            continue
        parts = line.split()
        peer = parts[4] if len(parts) > 4 else "?"
        if peer.startswith(LOCAL_PREFIXES):
            continue
        sent = recv = 0
        if i + 1 < len(lines):
            m = re.search(r"bytes_sent:(\d+)", lines[i + 1])
            sent = int(m.group(1)) if m else 0
            m = re.search(r"bytes_received:(\d+)", lines[i + 1])
            recv = int(m.group(1)) if m else 0
        result[peer] = (sent, recv)
    return result


def main() -> int:
    duration = float(sys.argv[1]) if len(sys.argv) > 1 else 60.0
    pid = resolve_pid(sys.argv[2] if len(sys.argv) > 2 else None)
    print(f"采样 pproxy-server pid={pid}，持续 {duration:.0f}s（每 200ms 一次）")

    baseline = snapshot(pid)
    peak: dict[str, tuple[int, int]] = dict(baseline)
    end = time.time() + duration
    while time.time() < end:
        for peer, (sent, recv) in snapshot(pid).items():
            cur = peak.get(peer, (0, 0))
            peak[peer] = (max(cur[0], sent), max(cur[1], recv))
        time.sleep(0.2)

    print(f"\n{'对端':44} {'sent 增量':>12} {'recv 增量':>12}")
    rows = []
    for peer, (sent, recv) in peak.items():
        base = baseline.get(peer, (0, 0))
        rows.append((sent - base[0] + recv - base[1], peer, sent - base[0], recv - base[1]))
    if not rows:
        print("  (采样期间无出站连接)")
    for total, peer, ds, dr in sorted(rows, reverse=True):
        print(f"{peer:44} {ds:>12} {dr:>12}   total={total}")
    print(
        "\n判读：流量最大的对端就是本次请求的出口。\n"
        "  Vercel/vgate → 76.76.21.x / 66.33.60.x 等（前端 IP）\n"
        "  CF gate      → 104.21.x / 172.67.x / 2606:4700:: 等 Cloudflare 段"
    )
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
