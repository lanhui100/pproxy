#!/usr/bin/env bash
# 校验「必须合规出口的 host 清单」在 Rust 数据面与 CF gate worker 两侧一致（ADR-008）。
#
# 两侧任一新增/删除 host 而另一侧未同步 → 非零退出。
# 用法: bash scripts/check-egress-parity.sh
set -euo pipefail
cd "$(dirname "$0")/.."

python3 - <<'PY'
import pathlib
import re
import sys

HOST_RE = r"['\"]([a-z0-9][a-z0-9.\-]*\.googleapis\.com)['\"]"

def parse(path, pattern):
    text = pathlib.Path(path).read_text(encoding="utf-8")
    m = re.search(pattern, text, re.S)
    if not m:
        sys.exit(f"FATAL: 未能从 {path} 解析 COMPLIANT_EGRESS_SUFFIXES")
    return re.findall(HOST_RE, m.group(1))

rust = parse(
    "crates/transport/src/route.rs",
    r"COMPLIANT_EGRESS_SUFFIXES[^=]*=\s*&?\[(.*?)\];",
)
worker = parse(
    "deploy/cf-gate-worker/gate-policy.mjs",
    r"COMPLIANT_EGRESS_SUFFIXES\s*=\s*\[(.*?)\]\n",
)

if not rust or not worker:
    sys.exit("FAIL: 任一侧清单为空")

if rust != worker:
    print("FAIL: 两侧合规出口 host 清单不一致")
    print("  rust  :", rust)
    print("  worker:", worker)
    sys.exit(1)

print(f"OK: 合规出口 host 清单一致 ({len(rust)} 项): " + ", ".join(rust))
PY
