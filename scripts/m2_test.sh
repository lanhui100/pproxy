#!/usr/bin/env bash
# m2_test.sh — M2 集成验证（spec §7）：纯 CLI 流程走通 pony 全命令。
#
# 隔离（R4 硬性）：临时 HOME（~/.pony/config.toml 落临时目录）+
# 数据面 18998 / 管理面 18899 / stub 18897，DB 与 config 均在 mktemp
# 临时目录，EXIT trap 清理。全程不触碰生产 pproxy 与生产 ~/.pony。
set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
DATA_ADDR="127.0.0.1:18998"
ADMIN_ADDR="127.0.0.1:18899"
STUB_PORT=18897

# 先用真实 HOME 构建二进制（临时 HOME 会让 cargo 找不到 ~/.cargo registry）
cargo build --quiet --bin pony --bin pproxy-server
BIN_DIR="$(pwd)/target/debug"

TMPDIR_M2="$(mktemp -d)"
SERVER_PID=""
STUB_PID=""
PONY="$BIN_DIR/pony"
trap 'kill "$SERVER_PID" "$STUB_PID" 2>/dev/null; rm -rf "$TMPDIR_M2"' EXIT

PASS=0
expect_eq() { # expect_eq actual expected label
  if [ "$1" = "$2" ]; then
    PASS=$((PASS + 1)); echo "  PASS: $3"
  else
    echo "  FAIL: $3"; echo "    expected: $2"; echo "    actual:   $1"; exit 1
  fi
}
expect_contains() { # expect_contains haystack needle label
  if [[ "$1" == *"$2"* ]]; then
    PASS=$((PASS + 1)); echo "  PASS: $3"
  else
    echo "  FAIL: $3"; echo "    haystack: $1"; echo "    needle:   $2"; exit 1
  fi
}
run_pony() { # run_pony label expected_code args...
  local label="$1" want="$2"; shift 2
  local out rc
  # set -e 下命令替换非零会直接终止脚本，须先短路捕获真实退出码
  out="$(env HOME="$HOME" "$PONY" "$@" 2>&1)" && rc=0 || rc=$?
  expect_eq "$rc" "$want" "$label (exit=$want)"
  printf '%s' "$out"
}

# ---- 生产保护断言 ----
prod_guard() {
  local state
  state="$(systemctl is-active pproxy 2>/dev/null || true)"
  expect_eq "$state" "active" "生产 pproxy 服务保持 active"
}
prod_guard

# ---- 准备：临时 server config + stub + 临时 HOME ----
echo "[准备] 构造临时 config + 启动 echo stub + 隔离 HOME"
export HOME="$TMPDIR_M2/home"
mkdir -p "$HOME"

PPROXY_DB="$TMPDIR_M2/state.db"
CONFIG="$TMPDIR_M2/config.json"
LOG="$TMPDIR_M2/server.log"

python3 - "$CONFIG" <<PYEOF
import json, sys
cfg = {
    "listen_host": "127.0.0.1",
    "listen_port": 18998,
    "static_upstreams": [],
    "countries": [],
    "worker_url": "https://edge.ponyjob.top",
    "worker_secret": "test-secret",
    "upstreams": {
        "vercel": {"url": "https://vercel.example/api/proxy", "secret": "s"},
        "localstub": {"url": "http://127.0.0.1:$STUB_PORT", "secret": "test-stub"}
    },
    "route_upstreams": {"echo": "localstub"},
    "pool_refresh_sec": 300
}
json.dump(cfg, open(sys.argv[1], "w"), indent=2)
PYEOF

export STUB_PORT
python3 "$SCRIPT_DIR/m1_echo_stub.py" &
STUB_PID=$!
sleep 0.5
curl -sS --max-time 5 -o /dev/null "http://127.0.0.1:$STUB_PORT/ping"

PPROXY_CONFIG="$CONFIG" PPROXY_DB="$PPROXY_DB" \
PPROXY_LISTEN_DATA="$DATA_ADDR" PPROXY_LISTEN_ADMIN="$ADMIN_ADDR" \
RUST_LOG=warn,pproxy_server=info \
  "$BIN_DIR/pproxy-server" >"$LOG" 2>&1 &
SERVER_PID=$!
for i in $(seq 1 60); do
  curl -sS --max-time 2 -o /dev/null "http://$DATA_ADDR/" 2>/dev/null && break
  sleep 0.5
done
curl -sS --max-time 5 -o /dev/null "http://$DATA_ADDR/"

ADMIN="$(grep -oP 'pony_admin_[0-9a-f]{48}' "$LOG" | head -1)"
[[ -n "$ADMIN" ]] || { echo "FAIL: 未提取到 ADMIN_TOKEN"; exit 1; }

# ---- 步骤 1：无配置 status → 退出码 2 ----
echo "[步骤 1] 无配置时 pony status → 退出码 2"
OUT="$($PONY status 2>&1)" && RC=0 || RC=$?
expect_eq "$RC" "2" "status 无配置退出码 2"
expect_contains "$OUT" "config not found" "stderr 含 config not found"

# ---- 步骤 2：init → 0，权限 600 ----
echo "[步骤 2] pony init"
run_pony "init 退出码 0" "0" init \
  --server "http://$ADMIN_ADDR" --token "$ADMIN" >/dev/null
MODE="$(stat -c '%a' "$HOME/.pony/config.toml")"
expect_eq "$MODE" "600" "config.toml 权限 600"

# ---- 步骤 3：status → 0 ----
echo "[步骤 3] pony status"
OUT="$(run_pony "status 退出码 0" "0" status)"
expect_contains "$OUT" "tokens_active" "输出含 tokens_active"

# ---- 步骤 4：route add gemini ----
echo "[步骤 4] pony route add gemini"
OUT="$(run_pony "route add gemini 退出码 0" "0" route add gemini generativelanguage.googleapis.com)"
expect_contains "$OUT" "created gemini" "输出含 created gemini"
expect_contains "$OUT" "upstream" "输出含 upstream 决策"

# ---- 步骤 5：route list 含 gemini ----
echo "[步骤 5] pony route list"
OUT="$(run_pony "route list 退出码 0" "0" route list)"
expect_contains "$OUT" "gemini" "列表含 gemini 行"

# ---- 步骤 6：stub 路由 + test ok=true ----
echo "[步骤 6] stub 路由（--upstream Named 绑定）+ route test"
run_pony "route add stub 退出码 0" "0" route add stub "stub.example.com" --upstream localstub >/dev/null
OUT="$(run_pony "route test stub 退出码 0" "0" route test stub)"
expect_contains "$OUT" ": ok" "test stub 输出 ok"

# ---- 步骤 7：disable 后 --all 不含 stub；enable 恢复 ----
echo "[步骤 7] route disable/enable"
run_pony "route disable stub" "0" route disable stub >/dev/null
OUT="$(run_pony "route test --all 退出码 0" "0" route test --all)"
if [[ "$OUT" == *"stub:"* ]]; then
  echo "  FAIL: disabled stub 不应出现在 test --all 输出"; exit 1
fi
PASS=$((PASS + 1)); echo "  PASS: test --all 不含 disabled stub"
run_pony "route enable stub" "0" route enable stub >/dev/null
OUT="$(run_pony "route test stub 恢复" "0" route test stub)"
expect_contains "$OUT" ": ok" "enable 后 stub 测试恢复 ok"

# ---- 步骤 8：token create + 数据面闭环 ----
echo "[步骤 8] pony token create + 明文捕获 + 数据面闭环"
OUT="$(run_pony "token create 退出码 0" "0" token create ci-token)"
PLAINTEXT="$(grep -oP 'pony_[0-9a-f]{32}' <<<"$OUT" | head -1)"
[[ -n "$PLAINTEXT" ]] || { echo "FAIL: 未捕获明文 token"; exit 1; }
PASS=$((PASS + 1)); echo "  PASS: 明文格式 ^pony_[0-9a-f]{32}$"
CODE="$(curl -sS --max-time 10 -o /dev/null -w '%{http_code}' "$DATA_ADDR/$PLAINTEXT/stub/ping")"
expect_eq "$CODE" "200" "数据面 /{token}/stub/ → 200（链路闭环）"

# ---- 步骤 9：token list 含 ci-token active ----
echo "[步骤 9] pony token list"
OUT="$(run_pony "token list 退出码 0" "0" token list)"
expect_contains "$OUT" "ci-token" "列表含 ci-token"
expect_contains "$OUT" "active" "状态含 active"

# ---- 步骤 10：数据面流量 → usage total.requests ≥ 已发数 ----
echo "[步骤 10] pony usage"
curl -sS --max-time 10 -o /dev/null "$DATA_ADDR/$PLAINTEXT/stub/ping"
curl -sS --max-time 10 -o /dev/null "$DATA_ADDR/$PLAINTEXT/stub/ping"
OUT="$(run_pony "usage 退出码 0" "0" usage --hours 1)"
TOTAL="$(grep -oP 'requests=\K[0-9]+' <<<"$OUT" | head -1)"
if [ "${TOTAL:-0}" -ge 3 ]; then
  PASS=$((PASS + 1)); echo "  PASS: usage total.requests≥3（actual=$TOTAL）"
else
  echo "  FAIL: usage total.requests=$TOTAL < 3"; echo "    out: $OUT"; exit 1
fi

# ---- 步骤 11：config export ----
echo "[步骤 11] pony config export"
OUT="$(run_pony "export openai 退出码 0" "0" config export openai --token "$PLAINTEXT")"
expect_contains "$OUT" "export OPENAI_BASE_URL=" "输出含 OPENAI_BASE_URL"
expect_contains "$OUT" "$PLAINTEXT" "输出嵌入明文"
run_pony "export nosuch 退出码 1" "1" config export nosuch >/dev/null

# ---- 步骤 12：doctor 通过；server 停后 status → 3 ----
echo "[步骤 12] pony doctor + 不可达场景"
OUT="$(run_pony "doctor 退出码 0" "0" doctor --probe-token "$PLAINTEXT")"
expect_contains "$OUT" "0 failed" "doctor failed: 0"
kill "$SERVER_PID" 2>/dev/null; wait "$SERVER_PID" 2>/dev/null || true
SERVER_PID=""
OUT="$($PONY status 2>&1)" && RC=0 || RC=$?
expect_eq "$RC" "3" "server 停止后 status 退出码 3"

# ---- 收尾 ----
prod_guard
echo ""
echo "=========================================="
echo "M2 集成测试全部通过：$PASS 项断言 PASS"
echo "=========================================="
