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

# C1 失败码契约：目标建连失败/超时 → 502（用 TEST-NET-1 192.0.2.0/24 做确定黑洞目标）
case_C1_502() {
  if ! _connect_precondition; then record C1-502 1 "$(_precond_detail)"; return; fi
  local out
  out=$(curl -s --noproxy '' -o /dev/null -m 30 -x "$PROXY_URL" -U "$TOKEN:$TOKEN" -v "https://192.0.2.1/" 2>&1)
  local detail
  detail=$(grep -iE 'response [0-9]+|HTTP/1\.[01] [0-9]{3}' <<<"$out" | head -3 | tr '\n' '|')
  if grep -qE '(response 502|HTTP/1\.[01] 502)' <<<"$out"; then
    record C1-502 0 "CONNECT 192.0.2.1:443（黑洞目标）→ 502（建连失败有界返回，预算 30s）"
  else
    record C1-502 1 "CONNECT 黑洞目标期望 502，实测片段: ${detail:-<无响应/连接失败>}"
  fi
}

# G4 — 生产端到端（公网 https://rn.ponygo.fun:8443 → 真实出海 204）。
#
# 凭据纪律（硬约束）：
#   * 令牌只从环境变量 PPROXY_E2E_TOKEN 读取，**不落盘、不打印、不进 argv**
#     （用 `curl -K -` 从 stdin 读配置，避免 token 出现在 `ps` 与任何日志里）；
#   * 未设置该 env 时本用例输出 [SKIP]，绝不猜测/复用其它凭据。
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
