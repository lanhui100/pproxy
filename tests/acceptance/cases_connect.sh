#!/usr/bin/env bash
# tests/acceptance/cases_connect.sh — 契约 C1（公网 CONNECT 数据面）验收用例。
#
# 覆盖门禁：
#   G1   合法 Basic 凭据 → 经本地 gate CONNECT 出网，目标返回 204
#   G1b  X-Pony-Token 凭据 → 同样出网 204（C1 双凭据口径）
#   G2   无凭据 → 407，且响应带 Proxy-Authenticate（C1 失败码契约）
#   G2b  错误凭据 → 407（覆盖"有凭据但无效"，防"任意凭据放行"）
#   G3   非 443 目标（example.com:80）→ 403（C1 目标 ACL）
#   C1-ACL 回环目标（127.0.0.1:443）→ 403（C1 is_valid_host 拒私网/回环）
#
# 不覆盖：G4（公网 rn.ponygo.fun:8443 端到端）——需要生产令牌 + 已生效 nginx
# stream 入口，超出本地 fixture 能力，见 README 豁免表。
#
# 环境纪律：curl 一律带 `--noproxy ''`，避免宿主 no_proxy 让 127.0.0.1 目标绕过代理
# 造成假绿/假红。

# 每个 CONNECT 用例的前置：数据面监听必须存在（C1：GATE_CONNECT_LISTEN 未设不启动，
# 设置后必须真的在监听）
_connect_precondition() {
  if [ "${CONNECT_UP:-0}" = "1" ]; then return 0; fi
  return 1
}

_precond_detail() {
  printf 'CONNECT 监听未就绪：GATE_CONNECT_LISTEN=%s:%s 无监听（契约 C1 未实现）' "$CONNECT_HOST" "$CONNECT_PORT"
}

case_G1() {
  if ! _connect_precondition; then record G1 1 "$(_precond_detail)"; return; fi
  local code
  code=$(curl -s --noproxy '' -o /dev/null -m "$HTTP_T" -w '%{http_code}' \
    -x "$PROXY_URL" -U "$TOKEN:$TOKEN" "$G1_TARGET" 2>/dev/null)
  if [ "$code" = "204" ]; then
    record G1 0 "CONNECT Basic(user=token,pass=token) → $G1_TARGET = 204"
  else
    record G1 1 "CONNECT Basic(user=token,pass=token) → $G1_TARGET 期望 204，实测 '${code:-<curl-error>}'"
  fi
}

case_G1b() {
  if ! _connect_precondition; then record G1b 1 "$(_precond_detail)"; return; fi
  local code
  code=$(curl -s --noproxy '' -o /dev/null -m "$HTTP_T" -w '%{http_code}' \
    -x "$PROXY_URL" --proxy-header "X-Pony-Token: $TOKEN" "$G1_TARGET" 2>/dev/null)
  if [ "$code" = "204" ]; then
    record G1b 0 "CONNECT X-Pony-Token → $G1_TARGET = 204"
  else
    record G1b 1 "CONNECT X-Pony-Token → $G1_TARGET 期望 204，实测 '${code:-<curl-error>}'"
  fi
}

# 407 断言：既看状态行，也看质询头（C1 明确要求 `Proxy-Authenticate: Basic realm="pproxy"`）
_assert_407() {
  local id="$1" out="$2" what="$3"
  local got_status=0 got_auth=0 detail
  grep -qE '(response 407|HTTP/1\.[01] 407)' <<<"$out" && got_status=1
  grep -qiE 'Proxy-Authenticate:[[:space:]]*Basic[[:space:]]+realm="?pproxy"?' <<<"$out" && got_auth=1
  detail=$(grep -iE 'response [0-9]+|Proxy-Authenticate:' <<<"$out" | head -3 | tr '\n' '|')
  if [ "$got_status" = "1" ] && [ "$got_auth" = "1" ]; then
    record "$id" 0 "$what → 407 + Proxy-Authenticate: Basic realm=\"pproxy\""
  else
    record "$id" 1 "$what → 期望 407 且带 Proxy-Authenticate（status_hit=$got_status auth_hit=$got_auth），实测片段: ${detail:-<无响应/连接失败>}"
  fi
}

case_G2() {
  if ! _connect_precondition; then record G2 1 "$(_precond_detail)"; return; fi
  local out
  out=$(curl -s --noproxy '' -o /dev/null -m "$HTTP_T" -x "$PROXY_URL" -v "$G1_TARGET" 2>&1)
  _assert_407 G2 "$out" "无凭据 CONNECT"
}

case_G2b() {
  if ! _connect_precondition; then record G2b 1 "$(_precond_detail)"; return; fi
  local out
  out=$(curl -s --noproxy '' -o /dev/null -m "$HTTP_T" -x "$PROXY_URL" -U "bad-user:bad-pass" -v "$G1_TARGET" 2>&1)
  _assert_407 G2b "$out" "错误凭据 CONNECT"
}

case_G3() {
  if ! _connect_precondition; then record G3 1 "$(_precond_detail)"; return; fi
  local out
  # -p / --proxytunnel：即使目标是 http(80) 也强制走 CONNECT，落实"非 443 → 403"
  out=$(curl -s --noproxy '' -o /dev/null -m "$HTTP_T" -p -x "$PROXY_URL" -U "$TOKEN:$TOKEN" -v "http://example.com:80/" 2>&1)
  local detail
  detail=$(grep -iE 'response [0-9]+|HTTP/1\.[01] [0-9]{3}' <<<"$out" | head -3 | tr '\n' '|')
  if grep -qE '(response 403|HTTP/1\.[01] 403)' <<<"$out"; then
    record G3 0 "CONNECT example.com:80 → 403（非 443 目标被 ACL 拒绝）"
  else
    record G3 1 "CONNECT example.com:80 期望 403，实测片段: ${detail:-<无响应/连接失败>}"
  fi
}

case_C1_ACL() {
  if ! _connect_precondition; then record C1-ACL 1 "$(_precond_detail)"; return; fi
  local out
  out=$(curl -s --noproxy '' -o /dev/null -m "$HTTP_T" -p -x "$PROXY_URL" -U "$TOKEN:$TOKEN" -v "http://127.0.0.1:443/" 2>&1)
  local detail
  detail=$(grep -iE 'response [0-9]+|HTTP/1\.[01] [0-9]{3}' <<<"$out" | head -3 | tr '\n' '|')
  if grep -qE '(response 403|HTTP/1\.[01] 403)' <<<"$out"; then
    record C1-ACL 0 "CONNECT 127.0.0.1:443 → 403（is_valid_host 拒绝回环/私网目标）"
  else
    record C1-ACL 1 "CONNECT 127.0.0.1:443 期望 403，实测片段: ${detail:-<无响应/连接失败>}"
  fi
}

# C1-BOUNDED — 不可达/被拒目标必须「有界返回」（C1 演进条款，2026-10-30 Lead 裁决）。
#
# 口径演进说明：原用例 `C1-502` 打 192.0.2.1（TEST-NET-1 文档段）期望 502；
# 新 ACL「目标必须是公网 IP」生效后该目标被正确拒绝为 403，故 502 不再是唯一正确码。
# 真正要钉的性质是「不无限挂起」：状态码 ∈ {403, 502} 且在 30s 预算内有返回。
case_C1_BOUNDED() {
  if ! _connect_precondition; then record C1-BOUNDED 1 "$(_precond_detail)"; return; fi
  local out1 out2 rc1=0 rc2=0
  # 1) 文档段目标（新 ACL 必拒）：192.0.2.1（TEST-NET-1）
  out1=$(run_helper "$HELPERS/raw_probe.py" probe \
    --connect-host "$CONNECT_HOST" --connect-port "$CONNECT_PORT" \
    --authority "192.0.2.1:443" --token "$TOKEN" --expect "403,502" --budget 30) || rc1=$?
  # 2) 已退役公网段（可能不可路由 → 502，或按 ACL 拒绝 → 403）
  out2=$(run_helper "$HELPERS/raw_probe.py" probe \
    --connect-host "$CONNECT_HOST" --connect-port "$CONNECT_PORT" \
    --authority "192.88.99.1:443" --token "$TOKEN" --expect "403,502" --budget 30) || rc2=$?
  if [ "$rc1" = "0" ] && [ "$rc2" = "0" ]; then
    record C1-BOUNDED 0 "不可达/被拒目标有界返回（码 ∈ {403,502}，≤30s）| $out1 | $out2"
  else
    record C1-BOUNDED 1 "不可达/被拒目标未在预算内返回 {403,502}（rc1=$rc1 rc2=$rc2）| $out1 | $out2"
  fi
}

# C1-ACL-ENCODINGS — ACL 编码绕过族（L3 安全审查 BLOCKING 的黑盒回归钉）。
# 全部变体都必须解析后判定为「非公网」并返回 403；任何 200/502/挂起都算失败。
ACL_BYPASS_AUTHORITIES=(
  "0.0.0.0:443"
  "2130706433:443"
  "0x7f000001:443"
  "[::ffff:127.0.0.1]:443"
  "[::]:443"
  "[0:0:0:0:0:0:0:1]:443"
  "100.64.0.1:443"
  "[fe80::1]:443"
  "10.0.0.1:443"
  "172.16.0.1:443"
  "192.168.1.1:443"
  "[fc00::1]:443"
)
case_C1_ACL_ENCODINGS() {
  if ! _connect_precondition; then record C1-ACL-ENCODINGS 1 "$(_precond_detail)"; return; fi
  local bad="" detail="" auth out rc
  for auth in "${ACL_BYPASS_AUTHORITIES[@]}"; do
    rc=0
    out=$(run_helper "$HELPERS/raw_probe.py" probe \
      --connect-host "$CONNECT_HOST" --connect-port "$CONNECT_PORT" \
      --authority "$auth" --token "$TOKEN" --expect "403" --budget 15) || rc=$?
    if [ "$rc" != "0" ]; then
      bad="$bad $auth→$(sed -n 's/.*status=\([^ ]*\).*/\1/p' <<<"$out")"
    fi
  done
  if [ -z "$bad" ]; then
    record C1-ACL-ENCODINGS 0 "12 个编码/私网/未指定/多播/CGNAT/链路本地目标全部 403（解析后判定，非字符串前缀）"
  else
    record C1-ACL-ENCODINGS 1 "以下目标未被 403 拒绝（编码绕过残留或非公网漏判）:${bad}"
  fi
}

case_G4() {
  if [ -z "${PPROXY_E2E_TOKEN:-}" ]; then
    skip G4 "需 PPROXY_E2E_TOKEN（生产令牌不入库、不入日志；未设即不执行）"
    return
  fi
  local code
  # 配置经 stdin 传入：proxy-user 只存在于本 shell 内存与 curl stdin
  code=$(printf 'proxy-user = "%s:%s"\n' "$PPROXY_E2E_TOKEN" "$PPROXY_E2E_TOKEN" | \
    curl -s -K - --noproxy '' -o /dev/null -m 30 -w '%{http_code}' \
      -x "https://rn.ponygo.fun:8443" "https://www.gstatic.com/generate_204" 2>/dev/null)
  if [ "$code" = "204" ]; then
    record G4 0 "生产端到端 https://rn.ponygo.fun:8443 → https://www.gstatic.com/generate_204 = 204"
  else
    record G4 1 "生产端到端 https://rn.ponygo.fun:8443 期望 204，实测 '${code:-<curl-error>}'"
  fi
}
