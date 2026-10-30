#!/usr/bin/env bash
# tests/governance/lib.sh — 治理修复验收套件公共原语（协议 / 记账 / 超时 / 探测约定）。
#
# 协议（stdout 每行机器可读，供 CI 与 Agent 直接解析）：
#   [PASS] <case-id> <实测 detail>
#   [FAIL] <case-id> <实测 detail>
#   [INFO] <纯观测>                     ← 不参与判定，仅提供取证上下文
#   末行：ACCEPT_EXIT=<0|1>，且进程退出码与之一致。
#
# 探测约定（全套件统一，probe_* / case_*）：
#   return 0    = 门禁条件成立（合规）→ 记 PASS
#   return != 0 = 违规 / 无法判定      → 记 FAIL
#   最后一行 stdout 必须是含真实观测值的单行 detail（禁止空话、禁止把异常吞成 0）。
#
# 纪律（对应契约矩阵）：
#   1. 本套件**不提供 SKIP**：无法验证即 FAIL（fail-closed），杜绝"没跑就当过"。
#   2. 任何外部命令都必须经 run_cmd / run_cmd_in 执行 —— 统一超时保护与退出码捕获，
#      禁止 `cmd | filter` 的退出码做断言（管道会让 rc 变成 filter 的 rc，历史假绿陷阱）。
#   3. 探测只读：除 $TMPROOT（仓库外临时目录）外不得写入任何文件。
#   4. 用例必须在 run.sh 的 REQUIRED_CASES 内被真实执行；未执行由 MISSING-CASE 兜底判 FAIL。

GOV_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
REPO_ROOT="$(cd "$GOV_DIR/../.." && pwd)"

PASS_N=0
FAIL_N=0
FAILED_IDS=""
EXECUTED_IDS=" "
# 单条外部命令的超时预算（秒）；超时按违规处理，不允许无限等待
CMD_T="${GOV_CMD_T:-90}"

CAP_OUT=""
CAP_RC=0

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
  return 0
}

executed() {
  case "$EXECUTED_IDS" in
    *" $1 "*) return 0 ;;
    *) return 1 ;;
  esac
}

# run_cmd <budget_s> <cmd...>
# 捕获 stdout+stderr 到 CAP_OUT、退出码到 CAP_RC。超时：timeout 返回 124。
run_cmd() {
  local t="$1"
  shift
  CAP_OUT="$(timeout -k 5 "$t" "$@" 2>&1)"
  CAP_RC=$?
  return 0
}

# run_cmd_in <dir> <budget_s> <cmd...> — 在指定工作目录执行（cd 失败即视为违规退出码）
run_cmd_in() {
  local d="$1" t="$2"
  shift 2
  CAP_OUT="$(cd "$d" 2>&1 && timeout -k 5 "$t" "$@" 2>&1)"
  CAP_RC=$?
  return 0
}

# oneline <text> — 折成单行 detail（截断到 200 字符）
oneline() {
  printf '%s' "$1" | tr '\n' ' ' | tr -s ' ' | cut -c1-200
}

# last_line <text> — 最后一条非空行
last_line() {
  printf '%s\n' "$1" | awk 'NF{l=$0} END{printf "%s", l}'
}

# count_lines <text> — 非空行数
count_lines() {
  printf '%s\n' "$1" | awk 'NF' | wc -l | tr -d ' '
}

# list_head <text> <max> — 取前 max 条非空行，单行拼接
list_head() {
  local max="${2:-5}" n
  n="$(printf '%s\n' "$1" | awk 'NF' | wc -l | tr -d ' ')"
  printf '%s\n' "$1" | awk 'NF' | head -n "$max" | awk '{printf "%s%s", (NR>1 ? " | " : ""), $0}'
  if [ "$n" -gt "$max" ]; then printf ' …(+%d)' "$((n - max))"; fi
  return 0
}

# run_probe <case-id> <probe-fn> [args...]
# 执行探测函数：透传其退出码做判定，把 [INFO] 行原样回显，最后一行作为 detail。
run_probe() {
  local id="$1" fn="$2"
  shift 2
  local out rc infos detail
  out="$("$fn" "$@" 2>&1)"
  rc=$?
  infos="$(printf '%s\n' "$out" | grep '^\[INFO\]' || true)"
  detail="$(printf '%s\n' "$out" | grep -v '^\[INFO\]' | awk 'NF{l=$0} END{printf "%s", l}')"
  if [ -n "$infos" ]; then printf '%s\n' "$infos"; fi
  if [ -z "$detail" ]; then detail="（probe 无 detail 输出；rc=$rc）"; fi
  record "$id" "$rc" "$detail"
  return 0
}
