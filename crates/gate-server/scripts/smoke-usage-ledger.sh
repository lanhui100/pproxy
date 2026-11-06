#!/usr/bin/env bash
# 用量账本冒烟：① SIGTERM 优雅退出强制落盘 ② 重启恢复同一账本 ③ 跨账单周期归档并归零
# 用法：bash crates/gate-server/scripts/smoke-usage-ledger.sh
set -euo pipefail

ROOT=$(cd "$(dirname "${BASH_SOURCE[0]}")/../../.." && pwd)
cd "$ROOT"
BIN=${BIN:-target/debug/pproxy-gate-server}
PORT=${PORT:-3998}

# 必须每次重建：仅当二进制缺失才构建会拿旧产物跑冒烟，测的不是当前代码。
cargo build -p pproxy-gate-server --bin pproxy-gate-server

TMPH=$(mktemp -d)
LEDGER="$TMPH/.pony/gate_usage.json"
fail() { echo "FAIL: $*" >&2; exit 1; }

run_until_term() { # $1=日志路径
  local log=$1 pid
  HOME="$TMPH" PORT="$PORT" GATE_BILLING_DAY=1 "$BIN" >"$log" 2>&1 &
  pid=$!
  sleep 2
  kill -TERM "$pid"
  wait "$pid" || fail "进程未优雅退出（见 $log）"
}

# ① 退出落盘
run_until_term "$TMPH/log1.txt"
grep -q "usage ledger flushed" "$TMPH/log1.txt" || fail "退出未打印落盘日志"
[[ -f "$LEDGER" ]] || fail "退出后账本缺失：$LEDGER"

# ② 重启恢复
run_until_term "$TMPH/log2.txt"
grep -q "restored usage from disk" "$TMPH/log2.txt" || fail "重启未恢复账本"

# ③ 跨周期归档并归零
printf '{"cycle_key":"2000-01","billing_day":1,"users":{"usr_old":4096},"updated_at":0}' > "$LEDGER"
run_until_term "$TMPH/log3.txt"
[[ -f "$TMPH/.pony/gate_usage_2000-01.json" ]] || fail "跨周期未归档上一周期账本"
# 账本以 to_string_pretty 落盘，空表形如 "users": {}
grep -q '"users": {}' "$LEDGER" || fail "跨周期未归零当前水位：$(cat "$LEDGER")"

echo "OK: 退出落盘 / 重启恢复 / 跨周期归档 全部通过（$TMPH）"
