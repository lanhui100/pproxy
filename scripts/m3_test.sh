#!/usr/bin/env bash
# m3_test.sh — M3 集成验证（spec §9）：监控轮询 + 告警 + 管理 API，全离线 stub 口径。
#
# 隔离（同 M2 模式）：临时 HOME/DB/config；数据面 18896 / 管理面 18895 /
# API stub 18894 / webhook stub 18893，与 M1/M2/生产端口零交集，EXIT trap 清理。
# R1 硬性：注入 dummy 凭据 PPROXY_CF_API_TOKEN=test 等——缺失任一则来源 disabled，
# stub 永不被触达。
set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
DATA_ADDR="127.0.0.1:18896"
ADMIN_ADDR="127.0.0.1:18895"
STUB_PORT=18894
HOOK_PORT=18893

# 先用真实 HOME 构建二进制（临时 HOME 会让 cargo 找不到 ~/.cargo registry）
cargo build --quiet --bin pproxy-server
BIN_DIR="$(pwd)/target/debug"

TMPDIR_M3="$(mktemp -d)"
SERVER_PID=""
STUB_PID=""
trap 'kill "$SERVER_PID" "$STUB_PID" 2>/dev/null; rm -rf "$TMPDIR_M3"' EXIT

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
json_assert() { # json_assert json_string python_expr(对 d 断言) label
  local label="$3"
  if python3 -c "
import json, sys
d = json.loads(sys.argv[1])
sys.exit(0 if ($2) else 1)
" "$1" 2>/dev/null; then
    PASS=$((PASS + 1)); echo "  PASS: $label"
  else
    echo "  FAIL: $label"; echo "    expr: $2"; echo "    json: $1"; exit 1
  fi
}
admin_get() { # admin_get path → body
  curl -sS --max-time 5 -H "Authorization: Bearer $ADMIN" "$1"
}
admin_post() { # admin_post path → (body)
  local out rc
  out="$(curl -sS --max-time 5 -X POST -H "Authorization: Bearer $ADMIN" -o /dev/stdout -w '\n%{http_code}' "$1")" && rc=0 || rc=$?
  printf '%s' "$out"
}

# ---- 生产保护断言 ----
prod_guard() {
  local state
  state="$(systemctl is-active pproxy 2>/dev/null || true)"
  expect_eq "$state" "active" "生产 pproxy 服务保持 active"
}
prod_guard

# ---- 准备：隔离 HOME + 双 stub ----
echo "[准备] 启动 CF/Vercel API stub + webhook 接收 stub + 隔离 HOME"
export HOME="$TMPDIR_M3/home"
mkdir -p "$HOME"

PPROXY_DB="$TMPDIR_M3/state.db"
CONFIG="$TMPDIR_M3/config.json"
LOG="$TMPDIR_M3/server.log"
SINK_FILE="$TMPDIR_M3/webhook_sink.jsonl"

cat > "$CONFIG" <<EOF
{
  "listen_host": "127.0.0.1",
  "listen_port": 18896,
  "static_upstreams": [],
  "countries": [],
  "worker_url": "https://edge.ponyjob.top",
  "worker_secret": "ci-secret",
  "upstreams": {},
  "route_upstreams": {},
  "pool_refresh_sec": 300
}
EOF

export STUB_PORT HOOK_PORT SINK_FILE
python3 "$SCRIPT_DIR/m3_stub.py" &
STUB_PID=$!
sleep 0.5
# 就绪探测走 API stub 的 /v1/usage（200），不污染 webhook sink 文件
curl -sS --max-time 5 -o /dev/null "http://127.0.0.1:$STUB_PORT/v1/usage"
echo "[准备] stub 就绪"

# ---- server 启动：R1 dummy 凭据 + 测试覆盖端点 + 2s 轮询 ----
echo "[启动] pproxy-server（POLL_INTERVAL_SEC=2，dummy 凭据）"
PPROXY_CONFIG="$CONFIG" PPROXY_DB="$PPROXY_DB" \
PPROXY_LISTEN_DATA="$DATA_ADDR" PPROXY_LISTEN_ADMIN="$ADMIN_ADDR" \
PPROXY_CF_API_TOKEN=test PPROXY_CF_ACCOUNT_TAG=test \
PPROXY_CF_GRAPHQL_URL="http://127.0.0.1:$STUB_PORT/graphql" \
PPROXY_VERCEL_TOKEN=test PPROXY_VERCEL_TEAM_ID=team_dummy \
PPROXY_VERCEL_API_BASE="http://127.0.0.1:$STUB_PORT" \
PPROXY_ALERT_WEBHOOK_URL="http://127.0.0.1:$HOOK_PORT/hook" \
PPROXY_POLL_INTERVAL_SEC=2 \
RUST_LOG=warn,pproxy_server=info \
  "$BIN_DIR/pproxy-server" >"$LOG" 2>&1 &
SERVER_PID=$!
for i in $(seq 1 60); do
  curl -sS --max-time 2 -o /dev/null "http://$DATA_ADDR/" 2>/dev/null && break
  sleep 0.5
done
curl -sS --max-time 5 -o /dev/null "http://$DATA_ADDR/" || true

ADMIN="$(grep -oP 'pony_admin_[0-9a-f]{48}' "$LOG" | head -1)"
[[ -n "$ADMIN" ]] || { echo "FAIL: 未提取到 ADMIN_TOKEN"; exit 1; }

# 首 tick 立即采集 + 至少一个完整周期后再断言（t≈0 与 t≈2 各采一轮）
sleep 4

# ---- 步骤 1：GET /api/quota —— cf pct≈85、sources 健康口径 ----
echo "[步骤 1] GET /api/quota：cf pct≈85 + sources.cf=ok + sources.vercel=unsupported_plan"
QUOTA="$(admin_get "http://$ADMIN_ADDR/api/quota")"
json_assert "$QUOTA" 'any(abs(s["pct"]-85.0)<0.1 and s["upstream"]=="cf" and s["metric"]=="requests_daily" and s["quota"]==100000 for s in d["snapshots"])' \
  "snapshots 含 cf requests_daily 且 pct≈85（85000/100000）"
json_assert "$QUOTA" '{s["state"] for s in d["sources"] if s["name"]=="cf"} == {"ok"}' \
  "sources.cf = ok"
json_assert "$QUOTA" '{s["state"] for s in d["sources"] if s["name"]=="vercel"} == {"unsupported_plan"}' \
  "sources.vercel = unsupported_plan（Hobby 降级口径）"
json_assert "$QUOTA" 'all(s.get("last_ok") is not None for s in d["sources"] if s["name"]=="cf")' \
  "sources.cf last_ok 非空"

# ---- 步骤 2：GET /api/alerts —— 非空且含 warning ----
echo "[步骤 2] GET /api/alerts：非空 + warning 级（阈值默认 80 < 85）"
ALERTS="$(admin_get "http://$ADMIN_ADDR/api/alerts")"
json_assert "$ALERTS" 'len(d["alerts"]) >= 1' "/api/alerts 非空"
json_assert "$ALERTS" 'any(a["level"]=="warning" for a in d["alerts"])' "含 warning 级条目"
json_assert "$ALERTS" 'any("cf requests_daily at 85.0% (85000/100000)" == a["message"] for a in d["alerts"])' \
  "message 人话格式钉住（不含凭据）"
json_assert "$ALERTS" 'd["alerts"][0]["id"] >= max(a["id"] for a in d["alerts"])' "倒序输出"

# ---- 步骤 3：webhook stub 收到 quota_alert POST ----
echo "[步骤 3] webhook 落盘文件含 quota_alert"
[[ -s "$SINK_FILE" ]] || { echo "FAIL: webhook sink 文件不存在或为空"; exit 1; }
expect_contains "$(cat "$SINK_FILE")" '"event":"quota_alert"' 'webhook body 含 "event":"quota_alert"'
json_assert "$(tail -1 "$SINK_FILE")" 'd["level"]=="warning"' "webhook body level=warning"

# ---- 步骤 4：持续轮询不重复告警（alerts 条数稳定）----
echo "[步骤 4] 再等 2+ 轮询周期，告警不重发"
COUNT1="$(python3 -c 'import json,sys; print(len(json.loads(sys.argv[1])["alerts"]))' "$ALERTS")"
sleep 5
ALERTS2="$(admin_get "http://$ADMIN_ADDR/api/alerts?limit=500")"
COUNT2="$(python3 -c 'import json,sys; print(len(json.loads(sys.argv[1])["alerts"]))' "$ALERTS2")"
expect_eq "$COUNT2" "$COUNT1" "持续轮询后 alerts 条数稳定为 $COUNT1（越线沿触发不重复）"
expect_eq "$COUNT1" "1" "首轮恰好 1 条告警（cf warning；vercel pct=-1 跳过不计）"

# ---- 步骤 5：read 幂等三态 + unread 过滤 ----
echo "[步骤 5] POST /api/alerts/{id}/read 幂等 + unread=1 过滤"
AID="$(python3 -c 'import json,sys; print(json.loads(sys.argv[1])["alerts"][0]["id"])' "$ALERTS")"
RESP="$(admin_post "http://$ADMIN_ADDR/api/alerts/$AID/read")"
expect_eq "$(tail -n1 <<<"$RESP")" "200" "首次标记已读 → 200"
RESP="$(admin_post "http://$ADMIN_ADDR/api/alerts/$AID/read")"
expect_eq "$(tail -n1 <<<"$RESP")" "200" "重复标记已读 → 200（R4 幂等）"
UNREAD="$(admin_get "http://$ADMIN_ADDR/api/alerts?unread=1&limit=500")"
json_assert "$UNREAD" 'd["alerts"] == []' "标记后 unread=1 过滤为空列表"
ALLROWS="$(admin_get "http://$ADMIN_ADDR/api/alerts?limit=500")"
json_assert "$ALLROWS" 'all(a["read_at"] is not None for a in d["alerts"])' "read_at 已落值"
RESP="$(admin_post "http://$ADMIN_ADDR/api/alerts/999999/read")"
expect_eq "$(tail -n1 <<<"$RESP")" "404" "不存在 id 标记已读 → 404"

# ---- 步骤 6：鉴权 —— 无 token 访问 /api/quota → 401 ----
echo "[步骤 6] 无 token 访问管理面新端点"
CODE="$(curl -sS --max-time 5 -o /dev/null -w '%{http_code}' "http://$ADMIN_ADDR/api/quota")"
expect_eq "$CODE" "401" "无 token GET /api/quota → 401"
CODE="$(curl -sS --max-time 5 -o /dev/null -w '%{http_code}' "http://$ADMIN_ADDR/api/alerts")"
expect_eq "$CODE" "401" "无 token GET /api/alerts → 401"
BODY401="$(curl -sS --max-time 5 "http://$ADMIN_ADDR/api/quota")"
expect_contains "$BODY401" '"unauthorized"' "401 文案沿用 ERR_UNAUTHORIZED"

# ---- 步骤 7：limit 钳制 ≤500 参数合法性 ----
echo "[步骤 7] limit 参数钳制"
OUT="$(admin_get "http://$ADMIN_ADDR/api/alerts?limit=99999")"
json_assert "$OUT" 'isinstance(d["alerts"], list)' "limit>500 不产生 5xx（服务端钳制）"

# ---- 收尾 ----
prod_guard
echo ""
echo "=========================================="
echo "M3 集成测试全部通过：$PASS 项断言 PASS"
echo "=========================================="
