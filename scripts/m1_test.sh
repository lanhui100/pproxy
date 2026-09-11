#!/usr/bin/env bash
# m1_test.sh — M1 集成回归（T8）：M1 spec §5 全部 8 步 + 裁决新增断言。
#
# 用法：
#   bash scripts/m1_test.sh                    # 在线模式（步骤 4/5 需外网）
#   M1_TEST_OFFLINE=1 bash scripts/m1_test.sh  # 离线门禁子集（4/5 SKIP-ONLINE）
#
# 隔离（T8 §3 硬性）：数据面 18999 / 管理面 18900 / stub 18901，
# DB 与 config 均在 mktemp 临时目录，EXIT trap 清理。全程不触碰生产。
set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
DATA_ADDR="127.0.0.1:18999"
ADMIN_ADDR="127.0.0.1:18900"
STUB_PORT=18901
OFFLINE="${M1_TEST_OFFLINE:-0}"

TMPDIR_M1="$(mktemp -d)"
SERVER_PID=""
STUB_PID=""
trap 'kill "$SERVER_PID" "$STUB_PID" 2>/dev/null; rm -rf "$TMPDIR_M1"' EXIT

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
db_query() { python3 -c "
import sqlite3, sys
c = sqlite3.connect(sys.argv[1])
for row in c.execute(sys.argv[2]):
    print('\t'.join(str(x) for x in row))
" "$PPROXY_DB" "$1"; }

# ---- 生产保护断言（脚本首尾各一次，T8 §8）----
prod_guard() {
  local state
  state="$(systemctl is-active pproxy 2>/dev/null || true)"
  expect_eq "$state" "active" "生产 pproxy 服务保持 active"
}
prod_guard

# ---- 步骤 0：前置——临时 config + 启动 stub ----
echo "[步骤 0] 构造临时 config + 启动 echo stub"
PPROXY_DB="$TMPDIR_M1/state.db"
CONFIG="$TMPDIR_M1/config.json"
LOG="$TMPDIR_M1/server.log"

python3 - "$CONFIG" <<'PYEOF'
import json, sys
cfg = {
    "listen_host": "127.0.0.1",
    "listen_port": 18999,
    "static_upstreams": [],
    "countries": [],
    "worker_url": "https://edge.example.com",
    "worker_secret": "test-secret",
    "routes": {
        "anthropic": "api.anthropic.com",
        "openai": "api.openai.com",
        "opencode": "opencode.ai",
        "google": "www.google.com",
        "github": "github.com",
        "x": "api.twitter.com",
        "facebook": "www.facebook.com",
        "echo": "echo.example.com"
    },
    "upstreams": {
        "vercel": {"url": "https://vercel.example/api/proxy", "secret": "s"},
        "localstub": {"url": "http://127.0.0.1:18901", "secret": "test-stub"}
    },
    "route_upstreams": {"openai": "vercel", "opencode": "vercel", "echo": "localstub"},
    "pool_refresh_sec": 300
}
json.dump(cfg, open(sys.argv[1], "w"), indent=2)
PYEOF

python3 "$SCRIPT_DIR/m1_echo_stub.py" &
STUB_PID=$!
sleep 0.5
curl -sS --max-time 5 -o /dev/null "http://127.0.0.1:$STUB_PORT/ping"
PASS=$((PASS + 1)); echo "  PASS: echo stub 就绪 (:$STUB_PORT)"

# ---- 步骤 1：启动 server，提取 ADMIN_TOKEN ----
echo "[步骤 1] 启动 server + ADMIN_TOKEN 引导"
PPROXY_CONFIG="$CONFIG" PPROXY_DB="$PPROXY_DB" \
PPROXY_LISTEN_DATA="$DATA_ADDR" PPROXY_LISTEN_ADMIN="$ADMIN_ADDR" \
RUST_LOG=warn,pproxy_server=info \
  cargo run --quiet --bin pproxy-server >"$LOG" 2>&1 &
SERVER_PID=$!

for i in $(seq 1 60); do
  curl -sS --max-time 2 -o /dev/null "http://$DATA_ADDR/" 2>/dev/null && break
  sleep 0.5
done
curl -sS --max-time 5 -o /dev/null "http://$DATA_ADDR/"

TOKEN_COUNT="$(grep -c 'pony_admin_[0-9a-f]\{48\}' "$LOG" || true)"
expect_eq "$TOKEN_COUNT" "1" "ADMIN_TOKEN 出现且恰一次"
ADMIN="$(grep -oP 'pony_admin_[0-9a-f]{48}' "$LOG" | head -1)"

# ---- 步骤 2：无 token → 401 同体 ----
echo "[步骤 2] 无 token 请求 → 401"
CODE="$(curl -sS --max-time 30 -o "$TMPDIR_M1/body" -w '%{http_code}' "$DATA_ADDR/anthropic/v1/messages")"
expect_eq "$CODE" "401" "无 token 返回 401"
expect_eq "$(cat "$TMPDIR_M1/body")" '{"error":"unauthorized"}' "401 body 固定文案"

# ---- 步骤 3：创建 token + name 重复 400 ----
echo "[步骤 3] 管理 API 创建 token"
RESP="$(curl -sS --max-time 30 -X POST "$ADMIN_ADDR/api/tokens" \
  -H "Authorization: Bearer $ADMIN" -H "Content-Type: application/json" \
  -d '{"name":"test-dev"}')"
TOKEN="$(echo "$RESP" | python3 -c 'import json,sys; print(json.load(sys.stdin)["token"])')"
[[ "$TOKEN" =~ ^pony_[0-9a-f]{32}$ ]] && { PASS=$((PASS+1)); echo "  PASS: token 格式 ^pony_[0-9a-f]{32}$"; } \
  || { echo "  FAIL: token 格式异常: $TOKEN"; exit 1; }
CODE="$(curl -sS --max-time 30 -o /dev/null -w '%{http_code}' -X POST "$ADMIN_ADDR/api/tokens" \
  -H "Authorization: Bearer $ADMIN" -H "Content-Type: application/json" \
  -d '{"name":"test-dev"}')"
expect_eq "$CODE" "400" "name 重复 → 400"

# ---- 步骤 3.5：CONNECT → 403（P0-1）----
# 注：curl 对 CONNECT 隧道被拒的固有行为是 exit 56（tunnel failed），
# %{http_code} 恒为 000，故经代理断言只验"隧道未建立"；403 状态行由
# 原始字节断言证明。
echo "[步骤 3.5] CONNECT 拦截"
PROXY_ERR="$(curl -sS -o /dev/null --proxy "http://$DATA_ADDR" https://example.com --max-time 10 2>&1 || true)"
expect_contains "$PROXY_ERR" "56" "CONNECT 经代理 → 隧道被拒（curl 56 tunnel failed）"
expect_contains "$PROXY_ERR" "403" "curl 错误信息含 403"
CONNECT_RESP="$(printf 'CONNECT example.com:443 HTTP/1.1\r\nHost: example.com:443\r\n\r\n' | timeout 5 nc 127.0.0.1 18999 || true)"
expect_contains "$CONNECT_RESP" "HTTP/1.1 403 Forbidden" "原始 CONNECT 字节 → 403 状态行且连接关闭"

# ---- 步骤 4：openai 真实链路（外网依赖）----
if [ "$OFFLINE" = "1" ]; then
  echo "[步骤 4] SKIP-ONLINE（离线模式）"
else
  echo "[步骤 4] openai 真实上游链路"
  BODY="$(curl -sS --max-time 30 "$DATA_ADDR/$TOKEN/openai/v1/models" -H "Authorization: Bearer sk-fake" || true)"
  CODE="$(curl -sS --max-time 30 -o /dev/null -w '%{http_code}' "$DATA_ADDR/$TOKEN/openai/v1/models" -H "Authorization: Bearer sk-fake" || true)"
  if [ "$CODE" = "000" ]; then
    echo "  FAIL: 上游不可达（外网问题）。外网不可达时可加 M1_TEST_OFFLINE=1 跑离线门禁子集"; exit 1
  fi
  expect_eq "$CODE" "401" "openai 上游 401"
  expect_contains "$BODY" "invalid_request_error" "openai body 含 invalid_request_error"
fi

# ---- 步骤 5：zen 真实链路（外网依赖）----
if [ "$OFFLINE" = "1" ]; then
  echo "[步骤 5] SKIP-ONLINE（离线模式）"
else
  echo "[步骤 5] zen（opencode→vercel）真实链路"
  BODY="$(curl -sS --max-time 30 -X POST "$DATA_ADDR/$TOKEN/opencode/zen/v1/chat/completions" \
    -H "Content-Type: application/json" \
    -d '{"model":"zen","messages":[{"role":"user","content":"ping"}]}' || true)"
  if [[ "$BODY" == *"CreditsError"* || "$BODY" == *"DataPolicyError"* ]]; then
    PASS=$((PASS + 1)); echo "  PASS: zen 链路通（上游业务错误即链路通）"
  else
    echo "  FAIL: zen body 无 CreditsError/DataPolicyError"; echo "    body: $BODY"; exit 1
  fi
fi

# ---- 步骤 5.5：x-pony-token 泄露断言（S-P1-1）----
echo "[步骤 5.5] x-pony-token 剥离端到端断言（localstub 回显）"
ECHO_RESP="$(curl -sS --max-time 30 "$DATA_ADDR/$TOKEN/echo/ping")"
HAS_TOKEN="$(echo "$ECHO_RESP" | python3 -c '
import json, sys
d = json.load(sys.stdin)
print("x-pony-token" in d["headers"])
')"
expect_eq "$HAS_TOKEN" "False" "转发头不含 x-pony-token（含 EdgeClient 全链路）"

# ---- 步骤 6：usage API ----
echo "[步骤 6] usage 查询"
USAGE="$(curl -sS --max-time 30 "$ADMIN_ADDR/api/usage?hours=1" -H "Authorization: Bearer $ADMIN")"
for field in hours since_hour rows total; do
  expect_contains "$USAGE" "\"$field\"" "usage 响应含 $field"
done
if [[ "$USAGE" == *"ts_hour"* ]]; then
  echo "  FAIL: rows 泄露 ts_hour 键（C-P2-10）"; exit 1
else
  PASS=$((PASS + 1)); echo "  PASS: rows 无 ts_hour 键（C-P2-10）"
fi
CODE="$(curl -sS --max-time 30 -o /dev/null -w '%{http_code}' "$ADMIN_ADDR/api/usage?hours=0" -H "Authorization: Bearer $ADMIN")"
expect_eq "$CODE" "400" "hours=0 → 400"
CODE="$(curl -sS --max-time 30 -o /dev/null -w '%{http_code}' "$ADMIN_ADDR/api/usage?hours=1000" -H "Authorization: Bearer $ADMIN")"
expect_eq "$CODE" "400" "hours=1000 → 400"
if [ "$OFFLINE" = "1" ]; then
  ECHO_ROW="$(echo "$USAGE" | python3 -c '
import json, sys
d = json.load(sys.stdin)
rows = [r for r in d["rows"] if r.get("route") == "echo"]
print(rows[0]["requests"] if rows else 0)
')"
  [ "$ECHO_ROW" -ge 1 ] && { PASS=$((PASS+1)); echo "  PASS: 离线模式 echo route requests≥1"; } \
    || { echo "  FAIL: usage 无 echo 流量记录"; exit 1; }
fi

# ---- 步骤 7：路由增删即时生效 ----
echo "[步骤 7] 路由热生效"
CODE="$(curl -sS --max-time 30 -o /dev/null -w '%{http_code}' -X POST "$ADMIN_ADDR/api/routes" \
  -H "Authorization: Bearer $ADMIN" -H "Content-Type: application/json" \
  -d '{"name":"m1test","target_host":"api.anthropic.com"}')"
expect_eq "$CODE" "201" "创建路由 m1test → 201"
CODE="$(curl -sS --max-time 30 -o /dev/null -w '%{http_code}' "$DATA_ADDR/$TOKEN/m1test/v1/messages" || true)"
# 热生效判据：非 404（unknown_route）即证明路由已被数据面读取。
# worker 上游真实可达时会透传其状态码（如 secret 错误的 403），同样算通。
[ "$CODE" != "404" ] && [ "$CODE" != "000" ] && { PASS=$((PASS+1)); echo "  PASS: m1test 生效（code=$CODE ≠ 404）"; } \
  || { echo "  FAIL: m1test 未生效 code=$CODE"; exit 1; }
CODE="$(curl -sS --max-time 30 -o /dev/null -w '%{http_code}' -X DELETE "$ADMIN_ADDR/api/routes/m1test" -H "Authorization: Bearer $ADMIN")"
expect_eq "$CODE" "200" "删除路由 m1test → 200"
BODY="$(curl -sS --max-time 30 "$DATA_ADDR/$TOKEN/m1test/v1/messages" || true)"
expect_contains "$BODY" '"error":"unknown_route"' "删除后请求 → unknown_route"

# ---- 步骤 8：撤销 token 即时生效 ----
echo "[步骤 8] token 撤销"
TOKEN_ID="$(python3 -c "
import json, sys
print(json.loads('''$RESP''')['id']
)" 2>/dev/null || db_query "select id from tokens where name='test-dev'")"
CODE="$(curl -sS --max-time 30 -o /dev/null -w '%{http_code}' -X DELETE "$ADMIN_ADDR/api/tokens/$TOKEN_ID" -H "Authorization: Bearer $ADMIN")"
expect_eq "$CODE" "200" "撤销 token → 200"
CODE="$(curl -sS --max-time 30 -o /dev/null -w '%{http_code}' "$DATA_ADDR/$TOKEN/echo/ping")"
expect_eq "$CODE" "401" "撤销后请求 → 401"

# ---- 步骤 9：迁移回归 ----
echo "[步骤 9] 迁移回归（8 行路由 + config 重写）"
# F8 修订：route_upstreams 绑定写入 override_upstream 列（resolve 实际读取列）；
# upstream 列为创建时快照，迁移无快照故全 NULL。
ROUTE_ROWS="$(db_query "select name, upstream, override_upstream from routes order by name")"
ROW_COUNT="$(echo "$ROUTE_ROWS" | grep -c . || true)"
expect_eq "$ROW_COUNT" "8" "routes 表恰 8 行（7 业务 + echo）"
up_col() { echo "$ROUTE_ROWS" | grep "^$1" | cut -f"$2"; }
for r in openai opencode google github x facebook anthropic echo; do
  expect_eq "$(up_col "$r" 2)" "None" "$r upstream 快照列 NULL（迁移无快照）"
done
expect_eq "$(up_col openai 3)" "vercel" "openai override_upstream=vercel"
expect_eq "$(up_col opencode 3)" "vercel" "opencode override_upstream=vercel"
expect_eq "$(up_col echo 3)" "localstub" "echo override_upstream=localstub"
python3 -c "
import json, sys
cfg = json.load(open('$CONFIG'))
assert 'routes' not in cfg, 'routes 键应删除'
for k in ('worker_url', 'worker_secret', 'upstreams', 'route_upstreams'):
    assert k in cfg, f'{k} 应保留'
assert 'db_path' in cfg, '应新增 db_path'
print('config 重写断言通过')
" && { PASS=$((PASS+1)); echo "  PASS: config.json 重写（删 routes、留凭据、加 db_path）"; } \
  || { echo "  FAIL: config 重写不符"; exit 1; }
BAK_HAS_ROUTES="$(python3 -c "
import json
cfg = json.load(open('$CONFIG.bak'))
print('routes' in cfg)
")"
expect_eq "$BAK_HAS_ROUTES" "True" ".bak 为原始内容（含 routes 键）"
for r in anthropic openai opencode google github x facebook; do
  CODE="$(curl -sS --max-time 30 -o /dev/null -w '%{http_code}' "$DATA_ADDR/$r/v1/x")"
  expect_eq "$CODE" "401" "业务路由 $r 无 token → 401（表加载完整）"
done

# ---- 步骤 10：旧端点下线 ----
echo "[步骤 10] 旧端点 /stats 下线"
CODE="$(curl -sS --max-time 30 -o /dev/null -w '%{http_code}' "$ADMIN_ADDR/stats" || true)"
[ "$CODE" != "200" ] && { PASS=$((PASS+1)); echo "  PASS: /stats 非 200（code=$CODE）"; } \
  || { echo "  FAIL: /stats 仍可达"; exit 1; }

# ---- 步骤 11：迁移后重启持久化 ----
echo "[步骤 11] 重启持久化"
kill "$SERVER_PID" 2>/dev/null; wait "$SERVER_PID" 2>/dev/null || true
LOG2="$TMPDIR_M1/server2.log"
PPROXY_CONFIG="$CONFIG" PPROXY_DB="$PPROXY_DB" \
PPROXY_LISTEN_DATA="$DATA_ADDR" PPROXY_LISTEN_ADMIN="$ADMIN_ADDR" \
RUST_LOG=warn,pproxy_server=info \
  cargo run --quiet --bin pproxy-server >"$LOG2" 2>&1 &
SERVER_PID=$!
for i in $(seq 1 60); do
  curl -sS --max-time 2 -o /dev/null "http://$DATA_ADDR/" 2>/dev/null && break
  sleep 0.5
done
curl -sS --max-time 5 -o /dev/null "http://$DATA_ADDR/"
COUNT2="$(grep -c 'pony_admin_[0-9a-f]\{48\}' "$LOG2" || true)"
expect_eq "$COUNT2" "0" "重启日志无第二条 ADMIN_TOKEN"
python3 -c "
import json
cfg = json.load(open('$CONFIG'))
assert 'routes' not in cfg
bak = open('$CONFIG.bak').read()
import json as j
assert 'routes' in j.loads(bak)
print('幂等断言通过')
" && { PASS=$((PASS+1)); echo "  PASS: AlreadyMigrated 幂等（config/bak 不再改写）"; } \
  || { echo "  FAIL: 幂等性破坏"; exit 1; }
RESP2="$(curl -sS --max-time 30 -X POST "$ADMIN_ADDR/api/tokens" \
  -H "Authorization: Bearer $ADMIN" -H "Content-Type: application/json" \
  -d '{"name":"post-restart"}')"
TOKEN2="$(echo "$RESP2" | python3 -c 'import json,sys; print(json.load(sys.stdin)["token"])')"
if [ "$OFFLINE" = "1" ]; then
  BODY="$(curl -sS --max-time 30 "$DATA_ADDR/$TOKEN2/openai/v1/models" || true)"
  expect_contains "$BODY" '"error":"upstream_error"' "离线：重启后转发链路 502 upstream_error（非 401/404）"
else
  CODE="$(curl -sS --max-time 30 -o /dev/null -w '%{http_code}' "$DATA_ADDR/$TOKEN2/openai/v1/models" -H "Authorization: Bearer sk-fake" || true)"
  [ "$CODE" = "401" ] && { PASS=$((PASS+1)); echo "  PASS: 重启后 openai 转发仍通（上游 401）"; } \
    || { echo "  FAIL: 重启后转发异常 code=$CODE"; exit 1; }
fi
CODE="$(curl -sS --max-time 30 -o /dev/null -w '%{http_code}' "$DATA_ADDR/$TOKEN2/echo/ping")"
expect_eq "$CODE" "200" "新 token echo 路由 200（localstub 链路重启后通）"

# ---- 收尾 ----
prod_guard
echo ""
echo "=========================================="
echo "M1 集成测试全部通过：$PASS 项断言 PASS"
[ "$OFFLINE" = "1" ] && echo "（离线模式：步骤 4/5 SKIP-ONLINE）"
echo "=========================================="
