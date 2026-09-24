#!/usr/bin/env bash
# .meta/gates/check-adr.sh - 检查 ADR 格式与目录两轴自洽性
set -euo pipefail

if [ "${1:-}" = "--help" ]; then
  echo "用法: check-adr.sh [文件...]"
  echo "验证 .agents/notes/ 下的决策记录是否符合两轴路径、命名及 Status 规范。"
  exit 0
fi

# 调用现成验证脚本
bash .agents/skills/write-adr/verify-note.sh "$@"
