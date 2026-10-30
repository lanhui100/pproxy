#!/usr/bin/env bash
# tests/governance/cases/g13_prepush.sh — G13-PREPUSH（Wave-2 追加）
#
# 冻结契约：
#   a) `.githooks/pre-push` 存在、可执行、`bash -n` 通过；
#   b) 其调用链包含 `tests/governance/run.sh`（或等价：`bash .meta/gates/secret-scan.sh` + `verify-note`）。
# 语义：本地最后一道闸在 **push 前**。pre-commit 可被 `--no-verify` 绕过，
#   pre-push 是对"已提交但未共享"的兜底；空壳（存在但什么都不调用）不算接线。

G13_PREPUSH_REL='.githooks/pre-push'

case_G13_PREPUSH() {
  local f="$REPO_ROOT/$G13_PREPUSH_REL" mode="" mode_show

  if [ ! -f "$f" ]; then
    printf '缺 %s（push 前无本地拦截层）\n' "$G13_PREPUSH_REL"
    return 1
  fi
  if [ ! -x "$f" ]; then
    printf '%s 不可执行（mode=%s，git 不会调用）\n' "$G13_PREPUSH_REL" "$(stat -c '%a' "$f" 2>/dev/null || printf '?')"
    return 1
  fi

  run_cmd "$CMD_T" bash -n "$f"
  if [ "$CAP_RC" -ne 0 ]; then
    printf '%s 语法检查失败（bash -n rc=%s）：%s\n' \
      "$G13_PREPUSH_REL" "$CAP_RC" "$(oneline "$CAP_OUT")"
    return 1
  fi

  if grep -qF -e 'tests/governance/run.sh' "$f"; then
    mode="governance-suite"
  elif grep -qF -e 'secret-scan.sh' "$f" && grep -qF -e 'verify-note' "$f"; then
    mode="secret-scan+verify-note"
  fi

  if [ -z "$mode" ]; then
    printf '%s 未调用 tests/governance/run.sh，也未同时调用 secret-scan.sh 与 verify-note（空壳钩子）\n' \
      "$G13_PREPUSH_REL"
    return 1
  fi

  printf '%s 存在且可执行、bash -n rc=0、拦截路径=%s\n' "$G13_PREPUSH_REL" "$mode"
  return 0
}
