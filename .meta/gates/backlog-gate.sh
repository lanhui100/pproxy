#!/usr/bin/env bash
# ============================================================
# backlog-gate.sh —— backlog 账本门（v3 批次日志 + 归档封存；v2 过渡期只读兼容）
#
# 语义（判定真源 = scripts/backlog.mjs，本脚本只编排，不复刻判据）：
#   v3（backlog/MODEL == v3）：lint（批次结构 + fold 不变量 + 租约）+ doctor（跨归档/租约对账）
#                              + verify-archive（归档批次 sha256 封存清单）
#   v2（仅 backlog/tasks.jsonl，尚无 MODEL）：lint + doctor 的**只读**兼容路径，并提示迁移；
#                              过渡期不拒绝（各仓按迁移指引自行升级，避免门禁逼死存量仓）
#   未启用账本：明确跳过
#   布局不一致（有 backlog/log/ 却缺 MODEL）：拒绝（fail-closed，不冒充通过）
#
# 退出码：0 放行 / 非零 拒绝。无 node → fail-closed。
# ============================================================
set -uo pipefail

ROOT="${PONYGO_WORKSPACE_ROOT:-$(git rev-parse --show-toplevel 2>/dev/null || pwd)}"
cd "$ROOT" || exit 1

MODEL_FILE="backlog/MODEL"
LEGACY_LEDGER="backlog/tasks.jsonl"
LOG_DIR="backlog/log"

if [ -f "$MODEL_FILE" ]; then
  IFS= read -r MODEL < "$MODEL_FILE" || MODEL=""
  if [ "$MODEL" != "v3" ]; then
    echo "backlog-gate: FAIL 未知账本模型 '$MODEL'（只认 v3）" >&2
    exit 1
  fi
  if ! command -v node >/dev/null 2>&1; then
    echo "backlog-gate: FAIL 无 node，账本一致性无法校验（fail-closed）" >&2
    exit 1
  fi
  if ! node scripts/backlog.mjs lint; then
    echo "backlog-gate: FAIL lint 未通过（批次/fold 不变量违规）" >&2
    exit 1
  fi
  if ! doctor_out=$(node scripts/backlog.mjs doctor 2>&1); then
    echo "backlog-gate: FAIL doctor 对账未通过：" >&2
    printf '%s\n' "$doctor_out" >&2
    exit 1
  fi
  if ! node scripts/backlog.mjs verify-archive; then
    echo "backlog-gate: FAIL verify-archive 未通过（归档封存清单不一致）" >&2
    exit 1
  fi
  echo "backlog-gate: PASS（v3：lint + doctor + verify-archive）"
  exit 0
fi

if [ -d "$LOG_DIR" ] && [ -n "$(ls -A "$LOG_DIR" 2>/dev/null)" ]; then
  echo "backlog-gate: FAIL 存在 backlog/log/ 但缺 backlog/MODEL（布局不一致，疑似迁移中断）" >&2
  exit 1
fi

if [ -f "$LEGACY_LEDGER" ]; then
  if ! command -v node >/dev/null 2>&1; then
    echo "backlog-gate: FAIL 无 node，账本一致性无法校验（fail-closed）" >&2
    exit 1
  fi
  if ! node scripts/backlog.mjs lint; then
    echo "backlog-gate: FAIL lint 未通过（v2 记录不变量违规）" >&2
    exit 1
  fi
  if ! doctor_out=$(node scripts/backlog.mjs doctor 2>&1); then
    echo "backlog-gate: FAIL doctor 对账未通过：" >&2
    printf '%s\n' "$doctor_out" >&2
    exit 1
  fi
  echo "backlog-gate: NOTE 仍是 v2 单文件账本（过渡期只读校验通过）；迁移指引见 backlog/README.md 与全局 pg-backlog skill" >&2
  echo "backlog-gate: PASS（v2 过渡：lint + doctor 只读兼容）"
  exit 0
fi

echo "backlog-gate: 跳过——账本尚未启用（无 backlog/MODEL 且无 backlog/tasks.jsonl）"
exit 0
