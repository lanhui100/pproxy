#!/usr/bin/env bash
# tests/acceptance/run.sh — pproxy「扫码 Clash 真的能出海」独立验收入口（Wave 4）。
#
# 合同唯一依据：.dev-team/contract.md（C1/C3/C4/C5/C6 + 门禁表 G1..G8）
# 覆盖矩阵与豁免登记：tests/acceptance/README.md
#
# 用法：
#   bash tests/acceptance/run.sh                 # 全量（构建 + 自建 fixture + 全部用例）
#   bash tests/acceptance/run.sh '^G5'           # 仅跑匹配正则的用例（调试；不做缺失校验）
#   ACCEPT_SKIP_BUILD=1 bash tests/acceptance/run.sh   # 复用已构建二进制
#   ACCEPT_KEEP=1 bash tests/acceptance/run.sh   # 保留临时目录（排障）
#
# 环境（全部本地自建，无外部凭据）：
#   ACCEPT_TOKEN    fixture 令牌，缺省 accept-fixture-token-9f2c
#   ACCEPT_HTTP_T   单请求超时秒，缺省 20
#
# 输出协议（机器可读，每行一条判定）：
#   [PASS] <case-id> <描述+实测值>
#   [FAIL] <case-id> <描述+实测值>
#   [SKIP] <case-id> <物理不可达原因>   ← 仅覆盖缺口，须在 README 豁免表登记
#   末尾：ACCEPT_EXIT=<0|1>
set -uo pipefail

ACCEPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
# shellcheck source=lib.sh
source "$ACCEPT_DIR/lib.sh"

FILTER="${1:-}"

TOKEN="${ACCEPT_TOKEN:-accept-fixture-token-9f2c}"
TOKEN_HASH="$(printf '%s' "$TOKEN" | sha256sum | awk '{print $1}')"
HTTP_T="${ACCEPT_HTTP_T:-20}"
DIRECT_T="${ACCEPT_DIRECT_T:-8}"

HTTP_HOST="127.0.0.1"
CONNECT_HOST="127.0.0.1"
HTTP_PORT="$(free_port)"
CONNECT_PORT="$(free_port)"
if [ "$HTTP_PORT" = "$CONNECT_PORT" ]; then CONNECT_PORT="$(free_port)"; fi
GATE_HTTP="http://$HTTP_HOST:$HTTP_PORT"
PROXY_URL="http://$CONNECT_HOST:$CONNECT_PORT"

# 契约 G1 字面量目标；可用 ACCEPT_G1_TARGET 覆盖（仅在宿主网络可达性不同时使用，
# 例如 google 被本机网络阻断时仍坚持验证同一语义的 generate_204 端点）
G1_TARGET_PRIMARY="${ACCEPT_G1_TARGET:-https://www.google.com/generate_204}"
G1_TARGET_FALLBACK="https://cp.cloudflare.com/generate_204"
G1_TARGET="$G1_TARGET_PRIMARY"

AT3_TARGET_HOST="cp.cloudflare.com"

GATE_BIN="$REPO_ROOT/target/debug/pproxy-gate-server"
CLI_BIN="$REPO_ROOT/target/debug/pproxy"
GATE_PID=""
WORKDIR=""
CONNECT_UP=0

REQUIRED_CASES="HARNESS-SELFTEST META-ASSERT-SENSITIVITY META-AT1-NEGCTRL G1 G1b G2 G2b G3 C1-ACL C1-502 G5a G5b G5c G5d G6-gate G7-gate G6-cli G7-cli G6-cli-failfast G8-static G6-desktop-static G7-desktop-static AT1a AT1b AT2a AT2b AT3"

CASE_TABLE=(
  "HARNESS-SELFTEST|case_HARNESS_SELFTEST"
  "G1|case_G1"
  "G1b|case_G1b"
  "G2|case_G2"
  "G2b|case_G2b"
  "G3|case_G3"
  "C1-ACL|case_C1_ACL"
  "C1-502|case_C1_502"
  "G4|case_G4"
  "G5a|case_G5a"
  "G5b|case_G5b"
  "G5c|case_G5c"
  "G5d|case_G5d"
  "G6-gate|case_G6_gate"
  "G7-gate|case_G7_gate"
  "G6-cli|case_G6_cli"
  "G7-cli|case_G7_cli"
  "G6-cli-failfast|case_G6_cli_failfast"
  "G8-static|case_G8_static"
  "G6-desktop-static|case_G6_desktop_static"
  "G7-desktop-static|case_G7_desktop_static"
  "AT1a|case_AT1a"
  "AT1b|case_AT1b"
  "AT2a|case_AT2a"
  "AT2b|case_AT2b"
  "AT3|case_AT3"
  "META-ASSERT-SENSITIVITY|case_META_ASSERT_SENSITIVITY"
  "META-AT1-NEGCTRL|case_META_AT1_NEGCTRL"
)

cleanup() {
  local rc=$?
  if [ -n "$GATE_PID" ] && kill -0 "$GATE_PID" 2>/dev/null; then
    kill "$GATE_PID" 2>/dev/null
    for _ in 1 2 3 4 5 6 7 8 9 10; do
      kill -0 "$GATE_PID" 2>/dev/null || break
      sleep 0.1
    done
    kill -9 "$GATE_PID" 2>/dev/null
    wait "$GATE_PID" 2>/dev/null
  fi
  if [ -n "$WORKDIR" ] && [ -d "$WORKDIR" ]; then
    if [ "${ACCEPT_KEEP:-0}" = "1" ]; then
      printf '[INFO] ACCEPT_KEEP=1，临时目录保留：%s\n' "$WORKDIR"
    else
      rm -rf "$WORKDIR"
    fi
  fi
  return $rc
}
trap cleanup EXIT INT TERM

summary() {
  local missing="" id
  if [ -z "$FILTER" ]; then
    for id in $REQUIRED_CASES; do
      executed "$id" || missing="$missing $id"
    done
    if [ -n "$missing" ]; then
      record MISSING-CASE 1 "必需用例未执行（无断言即放行的兜底门禁）:${missing}"
    fi
  fi

  printf '\n=== ACCEPT SUMMARY ===\n'
  printf 'FIXTURE gate_http=%s:%s connect=%s:%s token=<fixture> sha256=%s…\n' \
    "$HTTP_HOST" "$HTTP_PORT" "$CONNECT_HOST" "$CONNECT_PORT" "${TOKEN_HASH:0:12}"
  printf 'PASS=%d FAIL=%d SKIP=%d\n' "$PASS_N" "$FAIL_N" "$SKIP_N"
  if [ -n "$FAILED_IDS" ]; then printf 'FAILED_IDS=%s\n' "$FAILED_IDS"; fi
  if [ -n "$SKIPPED_IDS" ]; then printf 'SKIPPED_IDS=%s\n' "$SKIPPED_IDS"; fi
  if [ "$FAIL_N" -eq 0 ]; then
    printf 'ACCEPT_EXIT=0\n'
    return 0
  fi
  printf 'ACCEPT_EXIT=1\n'
  return 1
}

build_binaries() {
  if [ "${ACCEPT_SKIP_BUILD:-0}" = "1" ]; then
    note "ACCEPT_SKIP_BUILD=1：跳构建，直接使用 $GATE_BIN / $CLI_BIN"
  else
    info "cargo build -p pproxy-gate-server -p pproxy-cli"
    if ! ( cd "$REPO_ROOT" && cargo build -q -p pproxy-gate-server -p pproxy-cli ); then
      record BUILD 1 "cargo build 失败：被验收代码无法编译（红相）"
      return 1
    fi
  fi
  local b
  for b in "$GATE_BIN" "$CLI_BIN"; do
    if [ ! -x "$b" ]; then
      record BUILD 1 "二进制缺失或不可执行：$b"
      return 1
    fi
  done
  return 0
}

start_gate() {
  mkdir -p "$WORKDIR/home" "$WORKDIR/home-cli"
  # 全部凭据均为本地 fixture；GATE_CONNECT_LISTEN 是 C1 的开关（未设不启动）
  env -i \
    PATH="$PATH" \
    HOME="$WORKDIR/home" \
    RUST_LOG="warn" \
    HOST="$HTTP_HOST" \
    PORT="$HTTP_PORT" \
    TUNNEL_TOKEN_HASH="$TOKEN_HASH" \
    PROXY_SECRET="accept-fixture-proxy-secret" \
    GATE_ADMIN_TOKEN="accept-fixture-gate-admin" \
    GATE_CONNECT_LISTEN="$CONNECT_HOST:$CONNECT_PORT" \
    "$GATE_BIN" >"$WORKDIR/gate.log" 2>&1 &
  GATE_PID=$!

  if ! wait_http "http://$HTTP_HOST:$HTTP_PORT/debug" 20; then
    record BOOT 1 "gate-server 未在 20s 内就绪（$( kill -0 "$GATE_PID" 2>/dev/null && echo '进程存活但 HTTP 不响应' || echo '进程已退出')），日志尾部: $(tail -3 "$WORKDIR/gate.log" 2>/dev/null | tr '\n' ' ')"
    return 1
  fi
  info "gate-server 就绪 pid=$GATE_PID（HOME 隔离于 $WORKDIR/home）"

  if wait_tcp "$CONNECT_HOST" "$CONNECT_PORT" 5; then
    CONNECT_UP=1
    info "CONNECT 数据面已监听 $CONNECT_HOST:$CONNECT_PORT（C1）"
  else
    CONNECT_UP=0
    note "CONNECT 数据面未监听：GATE_CONNECT_LISTEN=$CONNECT_HOST:$CONNECT_PORT 被忽略（C1 未实现 → G1/G1b/G2/G2b/G3/C1-ACL/AT1*/AT2*/AT3 将如实报红）"
  fi
  return 0
}

select_egress_target() {
  local code
  code=$(direct_code "$G1_TARGET_PRIMARY")
  if [ "$code" = "204" ]; then
    info "G1 目标 = 契约字面量 $G1_TARGET_PRIMARY"
  else
    G1_TARGET="$G1_TARGET_FALLBACK"
    note "宿主网络直连 $G1_TARGET_PRIMARY 失败（实测 '${code:-timeout}'）→ G1 改用 $G1_TARGET_FALLBACK（同为 generate_204 语义、且属 C6 探活域）；这是网络环境替换，不是降低断言强度"
  fi
}

# 套件自身完整性：断言 pytest 式"辅助脚本失败会被吞"这类假绿不可能发生
case_HARNESS_SELFTEST() {
  if helper_selftest; then
    record HARNESS-SELFTEST 0 "run_helper 原样透传辅助脚本非零退出码（防 false-green 回归）"
  else
    record HARNESS-SELFTEST 1 "run_helper 吞掉了辅助脚本的非零退出码 —— 本套件断言可能假绿，必须先修 harness"
  fi
}

main() {
  WORKDIR="$(mktemp -d "${TMPDIR:-/tmp}/pproxy-accept.XXXXXX")"
  info "临时工作区 $WORKDIR（HOME / fixture env 全部隔离于此）"
  info "合同：$REPO_ROOT/.dev-team/contract.md"

  if [ -n "$FILTER" ]; then note "FILTER='$FILTER'：仅运行匹配用例（不做 REQUIRED_CASES 完备校验）"; fi
  # 用例文件在内存变量（WORKDIR/GATE_HTTP/PROXY_URL…）就绪后再 source：
  # 用例里的路径常量在 source 时需要这些变量，提前 source 会触发 set -u 失败。
  # shellcheck source=cases_connect.sh
  source "$ACCEPT_DIR/cases_connect.sh"
  # shellcheck source=cases_subscription.sh
  source "$ACCEPT_DIR/cases_subscription.sh"
  # shellcheck source=cases_cli.sh
  source "$ACCEPT_DIR/cases_cli.sh"
  # shellcheck source=cases_desktop_static.sh
  source "$ACCEPT_DIR/cases_desktop_static.sh"
  # shellcheck source=cases_adversarial.sh
  source "$ACCEPT_DIR/cases_adversarial.sh"
  # shellcheck source=cases_meta.sh
  source "$ACCEPT_DIR/cases_meta.sh"

  build_binaries || { summary; exit 1; }
  start_gate || { summary; exit 1; }
  select_egress_target

  local entry id fn
  for entry in "${CASE_TABLE[@]}"; do
    id="${entry%%|*}"
    fn="${entry##*|}"
    if [ -n "$FILTER" ] && ! [[ "$id" =~ $FILTER ]]; then continue; fi
    # 进程存活哨兵：被验收服务在用例之间死掉时立刻判红并附日志尾部，
    # 而不是让后续每条请求各自超时成 '000'（避免故障点漂移掩盖真因）
    if ! kill -0 "$GATE_PID" 2>/dev/null; then
      record "$id" 1 "gate-server 进程已退出（pid=$GATE_PID），本用例无法执行；日志尾部: $(tail -3 "$WORKDIR/gate.log" 2>/dev/null | tr '\n' ' ')"
      continue
    fi
    "$fn"
  done

  if summary; then exit 0; else exit 1; fi
}

main "$@"
