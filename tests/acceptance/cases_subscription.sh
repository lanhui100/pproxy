#!/usr/bin/env bash
# tests/acceptance/cases_subscription.sh — 契约 C5（订阅端点鉴权）+ C3/C6（gate-server 生成器口径）。
#
# 覆盖门禁：
#   G5a  /api/clash?token=probe（无效）→ 401，body 明示 invalid token
#   G5b  /clash（缺令牌）        → 401，body 明示 missing token
#   G5c  /api/clash?token=<有效> → 200，保留 subscription-userinfo /
#        profile-update-interval / content-disposition 三个订阅响应头
#   G5d  /clash?token=<有效>     → 200（别名端点同口径）
#   G6-gate  gate-server 生成器：proxies 含 port 8443 / username / password，
#            且不含 Authorization: "Bearer（C3）
#   G7-gate  gate-server 生成器：尾部 `- MATCH,PROXY`、GEOIP,CN,DIRECT 紧随其前、
#            探活域与内网 CIDR 全部排在最前 PROXY 规则之前（C6）
#
# 有效令牌 fixture：本地自建 gate-server 时注入 TUNNEL_TOKEN_HASH=sha256(TOKEN)，
# 因此"有效"判定走 C1/C5 的 tunnel token hash 路径，不需要任何外部凭据。

SUB_BODY="$WORKDIR/gate-clash.yaml"
SUB_HDR="$WORKDIR/gate-clash.headers"

case_G5a() {
  local body="$WORKDIR/g5a.body" code
  code=$(curl -s --noproxy '' -m "$HTTP_T" -o "$body" -w '%{http_code}' "$GATE_HTTP/api/clash?token=probe" 2>/dev/null)
  local bodyhead
  bodyhead=$(head -c 200 "$body" 2>/dev/null | tr '\n' ' ')
  if [ "$code" = "401" ] && grep -qi 'invalid token' "$body" 2>/dev/null; then
    record G5a 0 "/api/clash?token=probe → 401 且 body 明示 invalid token"
  else
    record G5a 1 "/api/clash?token=probe 期望 401+invalid token，实测 code='${code:-<curl-error>}' body='${bodyhead:-<空>}'"
  fi
}

case_G5b() {
  local body="$WORKDIR/g5b.body" code
  code=$(curl -s --noproxy '' -m "$HTTP_T" -o "$body" -w '%{http_code}' "$GATE_HTTP/clash" 2>/dev/null)
  local bodyhead
  bodyhead=$(head -c 200 "$body" 2>/dev/null | tr '\n' ' ')
  if [ "$code" = "401" ] && grep -qi 'missing token' "$body" 2>/dev/null; then
    record G5b 0 "/clash（缺令牌）→ 401 且 body 明示 missing token"
  else
    record G5b 1 "/clash（缺令牌）期望 401+missing token，实测 code='${code:-<curl-error>}' body='${bodyhead:-<空>}'"
  fi
}

case_G5c() {
  local code
  code=$(curl -s --noproxy '' -m "$HTTP_T" -D "$SUB_HDR" -o "$SUB_BODY" -w '%{http_code}' \
    "$GATE_HTTP/api/clash?token=$TOKEN" 2>/dev/null)
  local missing="" h
  for h in "subscription-userinfo" "profile-update-interval" "content-disposition"; do
    grep -qi "^$h:" "$SUB_HDR" 2>/dev/null || missing="$missing $h"
  done
  # 2026-10-30 演进：订阅正文含长期令牌 → 必须 no-store，禁中间缓存
  grep -qi '^cache-control:[[:space:]]*no-store' "$SUB_HDR" 2>/dev/null || missing="$missing cache-control:no-store"
  local size
  size=$(wc -c <"$SUB_BODY" 2>/dev/null || echo 0)
  if [ "$code" = "200" ] && [ -z "$missing" ] && [ "$size" -gt 0 ]; then
    record G5c 0 "/api/clash?token=<有效> → 200，订阅三头 + cache-control:no-store 齐全，body=${size}B"
  else
    record G5c 1 "/api/clash?token=<有效> 期望 200+订阅三头+no-store+非空 body，实测 code='${code:-<curl-error>}' size=${size}B missing_headers='${missing:-无}'"
  fi
}

case_G5d() {
  local body="$WORKDIR/g5d.body" code
  code=$(curl -s --noproxy '' -m "$HTTP_T" -o "$body" -w '%{http_code}' "$GATE_HTTP/clash?token=$TOKEN" 2>/dev/null)
  if [ "$code" = "200" ] && [ -s "$body" ]; then
    record G5d 0 "/clash?token=<有效> → 200（C5 别名端点同口径）"
  else
    record G5d 1 "/clash?token=<有效> 期望 200+非空 body，实测 code='${code:-<curl-error>}' size=$(wc -c <"$body" 2>/dev/null || echo 0)B"
  fi
}

case_G6_gate() {
  if [ ! -s "$SUB_BODY" ]; then
    record G6-gate 1 "无可用 YAML：订阅端点未返回 200 正文（依赖 G5c 红相），C3 无法验证"
    return
  fi
  local problems="" detail; local rc_fields=0
  grep -qE '^[[:space:]]*port:[[:space:]]*8443[[:space:]]*$' "$SUB_BODY" || problems="$problems port!=8443"
  grep -qE "^[[:space:]]*username:[[:space:]]*\"?$TOKEN\"?[[:space:]]*\$" "$SUB_BODY" || problems="$problems username-missing"
  grep -qE "^[[:space:]]*password:[[:space:]]*\"?$TOKEN\"?[[:space:]]*\$" "$SUB_BODY" || problems="$problems password-missing"
  grep -qiE 'Authorization:[[:space:]]*"?[[:space:]]*Bearer' "$SUB_BODY" && problems="$problems Bearer-forbidden"
  detail=$(run_helper "$HELPERS/yaml_contract.py" fields --file "$SUB_BODY" --token "$TOKEN") || rc_fields=$?
  if [ -z "$problems" ] && [ "$rc_fields" = "0" ]; then
    record G6-gate 0 "C3 口径: $detail"
  else
    record G6-gate 1 "C3 口径不符，problems='${problems:-无}' | $detail"
  fi
}

case_G7_gate() {
  if [ ! -s "$SUB_BODY" ]; then
    record G7-gate 1 "无可用 YAML：订阅端点未返回 200 正文（依赖 G5c 红相），C6 无法验证"
    return
  fi
  local detail rc=0
  detail=$(run_helper "$HELPERS/yaml_contract.py" rules --file "$SUB_BODY") || rc=$?
  if [ "$rc" = "0" ]; then
    record G7-gate 0 "C6 分流: $detail"
  else
    record G7-gate 1 "C6 分流不符: $detail"
  fi
}

# HEALTH-gate — 组自检 URL 口径（2026-10-30 演进）：https 204 端点、禁明文 cp.cloudflare.com
case_HEALTH_gate() {
  if [ ! -s "$SUB_BODY" ]; then
    record HEALTH-gate 1 "无可用 YAML（依赖 G5c 红相），健康检查 URL 无法验证"
    return
  fi
  local detail rc=0
  detail=$(run_helper "$HELPERS/yaml_contract.py" health --file "$SUB_BODY") || rc=$?
  if [ "$rc" = "0" ]; then
    record HEALTH-gate 0 "gate 生成器组自检: $detail"
  else
    record HEALTH-gate 1 "gate 生成器组自检不符: $detail"
  fi
}
