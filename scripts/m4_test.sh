#!/usr/bin/env bash
# m4_test.sh — M4 集成验证（spec docs/product/specs/m4/README.md §6）：
# 公网入口 smoke 测试（CF Tunnel）。
#
# 定位声明（R6）：请求从服务器发起，路径为「服务器→CF 边缘→隧道→服务器」自环，
# vantage 与手机 4G 不等价；真实验收锚点是 spec §7 手动项。
# 定性声明（R4）：本脚本为【生产变更型脚本】（创建/撤销 e2e token、启停隧道），
# 必须幂等可重跑；首尾 prod_guard；不新开监听端口。
set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
PUBLIC_URL="https://access.ponyjob.top"
METRICS_URL="http://127.0.0.1:19099"
DATA_ADDR="127.0.0.1:8899"
# M5 ADR-007 重绑后：管理面监听 tailnet 地址（经 TAILNET_ADMIN_BASE 注入，
# 形如 http://<TAILNET_IP>:8900；兼容旧名 M4_ADMIN_BASE；未设置时跳过管理面闭环）
ADMIN_BASE="${TAILNET_ADMIN_BASE:-${M4_ADMIN_BASE:-}}"

PASS=0
expect_eq() {
  if [ "$1" = "$2" ]; then PASS=$((PASS + 1)); echo "  PASS: $3"; else
    echo "  FAIL: $3"; echo "    expected: $2"; echo "    actual:   $1"; exit 1; fi
}
expect_contains() {
  if [[ "$1" == *"$2"* ]]; then PASS=$((PASS + 1)); echo "  PASS: $3"; else
    echo "  FAIL: $3"; echo "    haystack: $1"; echo "    needle:   $2"; exit 1; fi
}
prod_guard() {
  local state
  state="$(systemctl is-active pproxy 2>/dev/null || true)"
  expect_eq "$state" "active" "生产 pproxy 服务保持 active"
}

# ---- 步骤 0：生产保护 ----
prod_guard

# ---- 步骤 1：隧道服务状态 ----
echo "[步骤 1] pony-tunnel 服务状态"
expect_eq "$(systemctl is-active pony-tunnel.service)" "active" "pony-tunnel active"
expect_eq "$(systemctl is-enabled pony-tunnel.service)" "enabled" "pony-tunnel enabled（开机自启）"
expect_eq "$(systemctl show -p User --value pony-tunnel.service)" "dm" "unit 以 dm 运行（R2：防 service install 覆盖降级）"

# ---- 步骤 2：凭据权限（R2）----
echo "[步骤 2] 凭据链权限（dm 属主 + 仅属主可读）"
for f in "$HOME/.cloudflared/cert.pem" "$HOME"/.cloudflared/*.json "$HOME/.cloudflared/config.yml"; do
  PERM="$(stat -c '%U:%a' "$f")"
  case "$PERM" in
    dm:600|dm:400) PASS=$((PASS + 1)); echo "  PASS: $f = $PERM" ;;
    *) echo "  FAIL: $f 权限异常 = $PERM（应为 dm/600 或更严）"; exit 1 ;;
  esac
done

# ---- 步骤 3：DNS 双解析器回归（ADR-003 口径）----
echo "[步骤 3] DNS 双解析器"
ALI="$(dig +short @223.5.5.5 access.ponyjob.top A 2>/dev/null | grep -c '^[0-9]') "
SYS="$(dig +short access.ponyjob.top A 2>/dev/null | grep -c '^[0-9]')"
if [ "${ALI:-0}" -ge 1 ]; then PASS=$((PASS + 1)); echo "  PASS: 阿里 DNS 223.5.5.5 解析正常（敏感词过滤未复发）"; else
  echo "  FAIL: 阿里 DNS 解析为空"; exit 1; fi
if [ "${SYS:-0}" -ge 1 ]; then PASS=$((PASS + 1)); echo "  PASS: 系统解析器解析正常"; else
  echo "  FAIL: 系统解析为空"; exit 1; fi

# ---- 步骤 4：metrics 显式回环观测（F11/R6，非阻断）----
if curl -sS --max-time 3 -o /dev/null "$METRICS_URL/metrics" 2>/dev/null; then
  PASS=$((PASS + 1)); echo "  PASS: cloudflared metrics 绑定 127.0.0.1:19099 可读"
else
  echo "  WARN: metrics 不可读（非阻断观测项）"
fi
# ---- WARP 共存红线观测（R3/F1，非阻断）----
WARP_STATE="$(warp-cli status 2>/dev/null | head -1 || true)"
if [[ "$WARP_STATE" == *Disconnected* || -z "$WARP_STATE" ]]; then
  PASS=$((PASS + 1)); echo "  PASS: WARP 保持 Disconnected（共存红线未触发）"
else
  echo "  WARN: WARP 状态异常：$WARP_STATE（红线：connect 会卷入隧道出站连接）"
fi

# ---- 步骤 5：监听面隔离断言（R6：8899 与 8900 双回环）----
echo "[步骤 5] 监听面隔离"
BIND_8899="$(ss -tlnp 2>/dev/null | awk '$4 ~ /:8899$/ {print $4}' | head -1)"
BIND_8900="$(ss -tlnp 2>/dev/null | awk '$4 ~ /:8900$/ {print $4}' | head -1)"
# 受控地址运行时动态获取（tailnet 标识不入库）
TS_IP="$(tailscale ip -4 2>/dev/null | head -1 || true)"
expect_eq "$BIND_8899" "127.0.0.1:8899" "数据面仍仅绑 127.0.0.1（防公网直连绕过隧道）"
case "$BIND_8900" in
  "127.0.0.1:8900"|"${TS_IP:-__unset__}:8900") PASS=$((PASS + 1)); echo "  PASS: 管理面仅绑受控地址（$BIND_8900，ADR-007 口径）" ;;
  *) echo "  FAIL: 管理面监听异常 = $BIND_8900"; exit 1 ;;
esac

# ---- 步骤 6：公网闭环三重断言（R1）——无 token ----
echo "[步骤 6] 公网闭环：无 token → 精确 401 同体 + server: cloudflare"
BODY="$(curl -sS --max-time 15 -o /dev/stdout -D /tmp/m4_hdr "$PUBLIC_URL/openai/models")" && RC=0 || RC=$?
CODE="$(awk 'NR==1{print $2}' /tmp/m4_hdr)"
expect_eq "$RC" "0" "公网请求连通（curl 退出码 0）"
expect_eq "$CODE" "401" "无 token → 精确 401"
expect_eq "$BODY" '{"error":"unauthorized"}' "401 响应体同体文案（JSON 精确匹配）"
SRV="$(grep -i '^server:' /tmp/m4_hdr | tr -d '\r' | awk '{print $2}')"
expect_contains "$SRV" "cloudflare" "响应经 CF 边缘（server 头三重断言之第三重）"

# ---- 管理面闭环前置：步骤 7-11 需真实服务地址（tailnet 标识不入库，经环境注入）----
if [ -z "$ADMIN_BASE" ]; then
  echo ""
  echo "SKIP: 未设置 TAILNET_ADMIN_BASE（形如 http://<TAILNET_IP>:8900），跳过步骤 7-11 管理面闭环"
  echo ""
  echo "=========================================="
  echo "M4 集成测试（跳过模式）通过：$PASS 项断言 PASS（公网闭环已验，管理面闭环未执行）"
  echo "=========================================="
  exit 0
fi

# ---- 步骤 7：e2e token 全生命周期（R4：trap 兜底撤销 + 零回显）----
echo "[步骤 7] e2e token 创建 → 404 unknown_route 判据 → 撤销复验"
ADMIN="${PPROXY_ADMIN_TOKEN:-}"
if [ -z "$ADMIN" ]; then
  ADMIN="$(sudo journalctl -u pproxy.service --since "-24 hours" -o cat 2>/dev/null \
    | grep -oP 'pony_admin_[0-9a-f]{48}' | tail -1 || true)"
fi
[[ -n "$ADMIN" ]] || { echo "FAIL: 未获取到当前 admin token（重启后已轮换）；可 PPROXY_ADMIN_TOKEN=xxx 注入"; exit 1; }

E2E_NAME="m4-e2e-$(date +%s)"
CREATE_RESP="$(curl -sS --max-time 10 -X POST -H "Authorization: Bearer $ADMIN" \
  -H 'Content-Type: application/json' -d "{\"name\":\"$E2E_NAME\"}" $ADMIN_BASE/api/tokens)"
E2E_TOKEN="$(python3 -c 'import json,sys; print(json.loads(sys.argv[1]).get("token",""))' "$CREATE_RESP" 2>/dev/null || true)"
E2E_ID="$(python3 -c 'import json,sys; print(json.loads(sys.argv[1]).get("id",""))' "$CREATE_RESP" 2>/dev/null || true)"

cleanup() { # 幂等兜底：无论脚本从哪退出都尝试撤销 e2e token（零输出）
  if [ -n "${E2E_ID:-}" ] && [ -n "${ADMIN_SPILL_SAFE:-}" ]; then :; fi
  if [ -n "${E2E_ID:-}" ] && [ -n "${ADMIN:-}" ]; then
    curl -sS --max-time 10 -o /dev/null -X DELETE \
      -H "Authorization: Bearer ${M4_ADMIN_FOR_CLEANUP:-$ADMIN}" \
      $ADMIN_BASE/api/tokens/$E2E_ID 2>/dev/null || true
  fi
}
trap cleanup EXIT

[[ -n "$E2E_TOKEN" ]] || { echo "FAIL: e2e token 创建失败（不回显响应）"; exit 1; }
export M4_ADMIN_FOR_CLEANUP="$ADMIN"
PASS=$((PASS + 1)); echo "  PASS: e2e token 已创建（明文未落 stdout）"

# R1 核心判据：valid token + 不存在路由 → 网关层确定性 404 unknown_route
NBODY="$(curl -sS --max-time 15 -H "X-Pony-Token: $E2E_TOKEN" -o /dev/stdout -w '\n%{http_code}' "$PUBLIC_URL/nosuchroute-$(date +%s)/v1/x")"
NCODE="$(tail -n1 <<<"$NBODY")"; NBODY="$(sed '$d' <<<"$NBODY")"
expect_eq "$NCODE" "404" "有效 token + 不存在路由 → 404（穿透至网关路由层）"
expect_contains "$NBODY" '"error":"unknown_route"' "网关确定性错误体（弱判据否决后的核心断言，F13）"

# header 模式已在上一断言使用（X-Pony-Token），路径模式再验一次
PCODE="$(curl -sS --max-time 15 -o /dev/null -w '%{http_code}' "$PUBLIC_URL/$E2E_TOKEN/nosuchroute-x/v1/x")"
expect_eq "$PCODE" "404" "路径模式 /{token}/{route}/... 同判据"

# ---- 步骤 8：对照负例（R1：证明判据能分辨通/不通）----
echo "[步骤 8] 对照负例：停隧道必现 5xx，恢复后正例回归"
sudo systemctl stop pony-tunnel.service
sleep 2
FBODY="$(curl -sS --max-time 20 -o /dev/stdout -w '\n%{http_code}' "$PUBLIC_URL/openai/models" 2>/dev/null || printf '000')"
FCODE="$(tail -n1 <<<"$FBODY")"; FBODY="$(sed '$d' <<<"$FBODY")"
if [[ "$FCODE" =~ ^5 || "$FCODE" == "000" ]] && [[ "$FBODY" != *"unknown_route"* ]]; then
  PASS=$((PASS + 1)); echo "  PASS: 隧道停止后同一请求不再产生网关层判据（code=$FCODE，判据有效性成立）"
else
  echo "  FAIL: 停隧道后仍返回 code=$FCODE body=$FBODY —— 判据无法分辨通/不通"; exit 1
fi
sudo systemctl start pony-tunnel.service

# ---- 步骤 9：恢复 + 韧性（60s 轮询窗口，F15）----
echo "[步骤 9] 隧道恢复（≤60s 轮询）"
RECOVERED=""
for i in $(seq 1 30); do
  RC2="$(curl -sS --max-time 10 -o /dev/null -w '%{http_code}' "$PUBLIC_URL/openai/models" 2>/dev/null || printf '000')"
  if [ "$RC2" = "401" ]; then RECOVERED=yes; break; fi
  sleep 2
done
expect_eq "${RECOVERED:-no}" "yes" "隧道恢复且 401 正例回归（${i}×2s 内）"

# ---- 步骤 10：撤销复验闭环（R4）----
echo "[步骤 10] e2e token 撤销 + 401 复验"
curl -sS --max-time 10 -o /dev/null -X DELETE \
  -H "Authorization: Bearer $ADMIN" $ADMIN_BASE/api/tokens/$E2E_ID
VCODE="$(curl -sS --max-time 15 -o /dev/null -w '%{http_code}' "$PUBLIC_URL/$E2E_TOKEN/openai/models")"
expect_eq "$VCODE" "401" "撤销后经公网复验 → 401（token 生命周期闭环）"
E2E_TOKEN=""; E2E_ID=""  # 已撤销，交给 trap 时跳过

# ---- 步骤 11：环境幂等终检（F16）----
echo "[步骤 11] 环境幂等"
expect_eq "$(systemctl is-active pony-tunnel.service)" "active" "终检：隧道运行中"
LEFTOVER="$(curl -sS --max-time 10 -H "Authorization: Bearer $ADMIN" "$ADMIN_BASE/api/tokens" \
  | python3 -c 'import json,sys; print(sum(1 for t in json.load(sys.stdin)["tokens"] if t["name"].startswith("m4-e2e-") and t["status"] != "revoked"))')"
expect_eq "$LEFTOVER" "0" "终检：无遗留【活跃】m4-e2e-* token（已撤销历史行不计）"

# ---- 收尾 ----
prod_guard
echo ""
echo "=========================================="
echo "M4 集成测试全部通过：$PASS 项断言 PASS"
echo "=========================================="
