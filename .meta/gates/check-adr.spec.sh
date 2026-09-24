#!/usr/bin/env bash
# .meta/gates/check-adr.spec.sh - 负样本测试：验证 check-adr 能够非零拒绝非法 ADR
set -euo pipefail

TMP_DIR=$(mktemp -d)
trap 'rm -rf "$TMP_DIR"' EXIT

BAD_FILE="$TMP_DIR/bad-note.md"
cat << 'EOF' > "$BAD_FILE"
# Agent Note: 缺少 Status 和两轴路径

## Problem
Testing rejection.
EOF

# 验证单文件传给 check-adr.sh 会失败 (退出码非 0)
if bash .meta/gates/check-adr.sh "$BAD_FILE" >/dev/null 2>&1; then
  echo "FAIL: check-adr.sh 未能拒绝非法 ADR 样本" >&2
  exit 1
else
  echo "PASS: check-adr.sh 成功拦截非法 ADR 样本 (exit non-zero)"
  exit 0
fi
