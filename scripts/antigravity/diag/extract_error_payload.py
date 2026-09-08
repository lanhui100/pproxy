#!/usr/bin/env python3
"""从 Antigravity CLI 会话库提取底层错误载荷。

CLI 界面只显示 `Agent execution terminated due to error.` + `Error ID: <轨迹>-<步号>`，
真实响应（HTTP 状态、TraceID、JSON error、reason/error_number）只存在于会话库里。

用法:
    python3 extract_error_payload.py [会话目录] [扫描最近 N 个会话]
默认: ~/.gemini/antigravity-cli/conversations  /  10
"""
from __future__ import annotations

import glob
import os
import re
import sys
import time

MARKERS = [
    b"Agent execution terminated due to error.",
    b"FAILED_PRECONDITION",
    b"No capacity available",
    b"You do not have a valid license",
    b"Verify your account to continue",
]

TRACE_RE = re.compile(r"(TraceID|X-Cloudaicompanion-Trace-Id)\W{0,4}(0x)?([0-9a-f]{8,})", re.I)


def clean(raw: bytes) -> str:
    s = raw.decode("utf-8", "replace")
    s = re.sub(r"[^\x09\x0a\x0d\x20-\x7e]", " ", s)
    return re.sub(r" {3,}", " ", s).strip()


def main() -> int:
    root = os.path.expanduser(
        sys.argv[1] if len(sys.argv) > 1 else "~/.gemini/antigravity-cli/conversations"
    )
    limit = int(sys.argv[2]) if len(sys.argv) > 2 else 10

    files = sorted(glob.glob(os.path.join(root, "*.db")), key=os.path.getmtime, reverse=True)
    if not files:
        print(f"未找到会话库: {root}", file=sys.stderr)
        return 1

    found = 0
    for path in files[:limit]:
        data = open(path, "rb").read()
        hits = [(m, data.rfind(m)) for m in MARKERS]
        hits = [(m, i) for m, i in hits if i >= 0]
        if not hits:
            continue
        found += 1
        _, idx = max(hits, key=lambda kv: kv[1])
        window = clean(data[max(0, idx - 240) : idx + 900])
        mtime = time.strftime("%H:%M:%S", time.localtime(os.path.getmtime(path)))
        print(f"### {os.path.basename(path)}  (mtime {mtime})")
        for line in window.splitlines():
            if line.strip():
                print("   ", line)
        traces = sorted({m.group(3) for m in TRACE_RE.finditer(window)})
        if traces:
            print("    TraceID:", ", ".join(traces))
        print("=" * 100)

    if not found:
        print(f"最近 {limit} 个会话中未发现已知错误标记。")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
