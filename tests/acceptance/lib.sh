#!/usr/bin/env bash
# tests/acceptance/lib.sh — 独立验收套件公共原语（端口分配 / 就绪轮询 / 断言记账）。
#
# 纪律（与 .dev-team/contract.md 对齐）：
#   1. 本文件不含任何业务判断；每条断言必须由用例函数显式 `record`，
#      未经 `record` 的用例会被 run.sh 的 REQUIRED_CASES 校验判为缺失（非零退出）。
#   2. 就绪轮询只用于"等被拉起"，不用于断言等待；断言一律给固定超时预算。
#   3. 失败必须带实测值，禁止吞异常放行。

ACCEPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
REPO_ROOT="$(cd "$ACCEPT_DIR/../.." && pwd)"
HELPERS="$ACCEPT_DIR/helpers"

PASS_N=0
FAIL_N=0
SKIP_N=0
FAILED_IDS=""
EXECUTED_IDS=" "
SKIPPED_IDS=""

note() { printf '[NOTE] %s\n' "$*"; }
info() { printf '[INFO] %s\n' "$*"; }

# record <case-id> <rc 0|1> <detail...>
record() {
  local id="$1" rc="$2"
  shift 2
  EXECUTED_IDS="$EXECUTED_IDS$id "
  if [ "$rc" -eq 0 ]; then
    PASS_N=$((PASS_N + 1))
    printf '[PASS] %s %s\n' "$id" "$*"
  else
    FAIL_N=$((FAIL_N + 1))
    FAILED_IDS="$FAILED_IDS$id "
    printf '[FAIL] %s %s\n' "$id" "$*"
  fi
}

# skip 仅用于"本机物理不可达"的覆盖缺口（必须在 README 豁免表登记，且 summary 显式列出）
skip() {
  SKIP_N=$((SKIP_N + 1))
  SKIPPED_IDS="$SKIPPED_IDS$1 "
  printf '[SKIP] %s %s\n' "$1" "$2"
}

executed() {
  case "$EXECUTED_IDS" in
    *" $1 "*) return 0 ;;
    *) return 1 ;;
  esac
}

# 从内核拿一个空闲回环端口（bind(0) 后立即释放）
free_port() {
  python3 - <<'PY'
import socket
s = socket.socket()
s.bind(("127.0.0.1", 0))
print(s.getsockname()[1])
s.close()
PY
}

# wait_http <url> <budget_s> — 轮询直到 HTTP 200
wait_http() {
  local url="$1" budget="${2:-20}" start now code
  start=$(date +%s%N)
  while :; do
    code=$(curl -s --noproxy '*' -o /dev/null -m 2 -w '%{http_code}' "$url" 2>/dev/null)
    [ "$code" = "200" ] && return 0
    now=$(date +%s%N)
    [ $(((now - start) / 1000000)) -ge $((budget * 1000)) ] && return 1
    sleep 0.1
  done
}

# wait_tcp <host> <port> <budget_s> — 轮询直到 TCP 可建连
wait_tcp() {
  local host="$1" port="$2" budget="${3:-10}" start now
  start=$(date +%s%N)
  while :; do
    if timeout 1 bash -c "exec 3<>/dev/tcp/$host/$port" 2>/dev/null; then return 0; fi
    now=$(date +%s%N)
    [ $(((now - start) / 1000000)) -ge $((budget * 1000)) ] && return 1
    sleep 0.1
  done
}

# 直连（绕过代理）探测某 URL 的状态码；用于判定宿主网络是否可达（环境前置，不作为契约断言）
direct_code() {
  curl --noproxy '*' -s -o /dev/null -m "${DIRECT_T:-8}" -w '%{http_code}' "$1" 2>/dev/null
}

# 运行 python 辅助脚本并折叠为单行 detail；**必须原样透传 helper 的退出码**
# （历史陷阱：管道到 tr 会让 $? 变成 tr 的状态，导致 helper 失败被吞成假绿。
#  约束：任何用例都不得用 `$(cmd | filter)` 的退出码做断言。）
run_helper() {
  local out rc
  out=$(python3 "$@" 2>&1)
  rc=$?
  printf '%s' "$(printf '%s' "$out" | tr '\n' ' ')"
  return $rc
}

# 自检：确认 run_helper 真的透传非零退出码（防止"false green"回归）
helper_selftest() {
  run_helper /bin/false >/dev/null 2>&1 && return 1
  return 0
}
