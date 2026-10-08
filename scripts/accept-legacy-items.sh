#!/usr/bin/env bash
# 冻结红相 → 绿相机器验收脚本（legacy 三缺陷 A/B/C）。
#
# 用法:
#   bash scripts/accept-legacy-items.sh                  # A/B 必过；C 按 deploy-deferred 跳过
#   bash scripts/accept-legacy-items.sh --with-deploy    # A/B/C 全量（需已完成 worker.js 标记头 + 凭据重部署）
#
# 断言契约:
#   A) pproxy doctor 输出须含 `[pass] CONNECT tunnel probe: oauth2.googleapis.com:443 → 200`
#      （doctor CONNECT 探针缺 Host 头修复后网关回 200）
#   B) pproxy status 输出须匹配 `systemd (pproxy [system]): active` 或
#      `systemd (pproxy-server [system]): active`（systemd 单元名漂移修复后识别系统级单元）
#   C) 部署后 POST /api/routes/{anthropic,github}/test 应 ok:true（admin Bearer 凭据取自
#      ~/.pony/config.toml 的 admin_token；worker.js 标记头 + 凭据重部署未完成时显式跳过）
#
# 退出码: 0 = 全部通过（或 C 显式跳过）；1 = A/B/C 任一失败；2 = 用法错误。
# 环境变量: PPROXY_BIN — 覆盖 pproxy 二进制路径（默认 `pproxy`）。绿相验证须指向本波次
# 新构建的 ./target/release/pproxy（PATH 上的 ~/.local/bin/pproxy 为旧二进制，无本波次修复）。
set -euo pipefail

PPROXY_BIN="${PPROXY_BIN:-pproxy}"

WITH_DEPLOY=0
for a in "$@"; do
  case "$a" in
    --with-deploy) WITH_DEPLOY=1 ;;
    *) echo "unknown arg: $a" >&2; exit 2 ;;
  esac
done

fail() { echo "ACCEPT-FAIL: $1" >&2; exit 1; }

# ---- A) doctor CONNECT 隧道探针（Host 头修复） ----
echo "== A) pproxy doctor CONNECT tunnel probe =="
A_OUT="$("$PPROXY_BIN" doctor 2>&1 || true)"
A_LINE="$(printf '%s\n' "$A_OUT" | grep -E '^\[pass\] CONNECT tunnel probe: oauth2\.googleapis\.com:443 → 200$' || true)"
if [ -z "$A_LINE" ]; then
  printf '%s\n' "$A_OUT" | grep -E '^\[(pass|fail)\] CONNECT tunnel probe:' >&2 || true
  fail "A: CONNECT 探针未达 [pass] ... → 200（当前为 FAIL）"
fi
echo "A PASS: $A_LINE"

# ---- B) status systemd 行（系统级单元识别） ----
echo "== B) pproxy status systemd =="
B_OUT="$("$PPROXY_BIN" status 2>&1 || true)"
B_LINE="$(printf '%s\n' "$B_OUT" | grep -E '^systemd \((pproxy|pproxy-server) \[system\]\): active$' || true)"
if [ -z "$B_LINE" ]; then
  printf '%s\n' "$B_OUT" | grep -E '^systemd \(' >&2 || true
  fail "B: status 未识别到 system 级 active 单元（当前为 FAIL）"
fi
echo "B PASS: $B_LINE"

# ---- C) 部署后路由 test（deploy-deferred：本环境无 CF 凭据，重部署不可行） ----
if [ "$WITH_DEPLOY" -ne 1 ]; then
  echo "C SKIP: [RED DEGRADED: DEPLOY-DEFERRED] 未带 --with-deploy（worker.js 标记头 + 凭据重部署未完成，跳过）"
  exit 0
fi

echo "== C) admin API /api/routes/{anthropic,github}/test =="
CFG="$HOME/.pony/config.toml"
[ -r "$CFG" ] || fail "C: 找不到 $CFG"
TOKEN="$(sed -n 's/^admin_token[[:space:]]*=[[:space:]]*"\([^"]*\)".*/\1/p' "$CFG" | head -1)"
[ -n "$TOKEN" ] || fail "C: $CFG 中无 admin_token"
for name in anthropic github; do
  RESP="$(curl -sS -m 30 -X POST "http://100.95.193.103:8900/api/routes/$name/test" -H "Authorization: Bearer $TOKEN" 2>&1)" || fail "C: curl $name 失败: $RESP"
  OK="$(printf '%s' "$RESP" | sed -n 's/.*"ok"[[:space:]]*:[[:space:]]*\(true\|false\).*/\1/p' | head -1)"
  if [ "$OK" != "true" ]; then
    echo "C FAIL: $name → $RESP" >&2
    exit 1
  fi
  echo "C PASS: $name → ok:true ($RESP)"
done

echo "ALL ACCEPT PASS"
