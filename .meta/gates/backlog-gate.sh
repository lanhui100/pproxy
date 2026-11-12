#!/usr/bin/env bash
# ============================================================
# backlog-gate.sh —— backlog 账本门（记录不变量 + 跨归档/租约对账）
#
# 语义 = node scripts/backlog.mjs lint + doctor：
#   - lint   记录级强校验（必填字段 / 条件必填 / 状态字段合法性 / 租约-状态互斥 / 活跃池 done 残留 / ID 唯一 / 高水位）
#   - doctor 跨归档对账（ID 重复 / 高水位 / 孤儿租约 / 活跃池 done / 租约-状态互斥）
#
# 退出码：0 放行 / 非零 拒绝
# 判定真源：scripts/backlog.mjs（本脚本只编排，不复刻判据）
#
# 迁移期语义：backlog/tasks.jsonl 不存在 → 明确提示后跳过（尚未启用账本）。
# 无 node → fail-closed（门无法判定即不冒充通过）。
# ============================================================
set -uo pipefail

ROOT="${PONYGO_WORKSPACE_ROOT:-$(git rev-parse --show-toplevel 2>/dev/null || pwd)}"
cd "$ROOT" || exit 1

LEDGER="backlog/tasks.jsonl"

if [ ! -f "$LEDGER" ]; then
  echo "backlog-gate: 跳过——$LEDGER 不存在（账本尚未启用）"
  exit 0
fi

if ! command -v node >/dev/null 2>&1; then
  echo "backlog-gate: FAIL 无 node，账本一致性无法校验（fail-closed）" >&2
  exit 1
fi

if ! node scripts/backlog.mjs lint; then
  echo "backlog-gate: FAIL lint 未通过（记录不变量违规）" >&2
  exit 1
fi

if ! doctor_out=$(node scripts/backlog.mjs doctor 2>&1); then
  echo "backlog-gate: FAIL doctor 对账未通过：" >&2
  printf '%s\n' "$doctor_out" >&2
  exit 1
fi

echo "backlog-gate: PASS（lint + doctor 对账通过）"
exit 0
