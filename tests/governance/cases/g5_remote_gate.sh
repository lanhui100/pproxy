#!/usr/bin/env bash
# tests/governance/cases/g5_remote_gate.sh — G5-REMOTE-GATE（契约修订版：传递闭包判定）
#
# 冻结契约（Lead 修订，替代"workflow 字面含 upgrade --check"）：
#   a) `.github/workflows/ponygo-governance.yml` 存在；
#   b) 该 workflow 调用 `.meta/gates/ci-verify.sh`（薄适配器，ponygo 二进制不装进 CI）；
#   c) `.meta/gates/ci-verify.sh` 内含 `verify-note.sh` 调用（闭包终点：CI 真的会校验决策记录）。
# 任一跳缺失即"闭包断点"→ FAIL（远端层看起来接线了、实际什么都没校验 = 典型假绿）。

WF_REL='.github/workflows/ponygo-governance.yml'
CI_VERIFY_REL='.meta/gates/ci-verify.sh'

# 统计文件中某字面量的命中行数（无命中输出 0）
count_literal() {
  local f="$1" lit="$2" n
  n="$(grep -cF -e "$lit" "$f" 2>/dev/null || true)"
  printf '%s' "${n:-0}"
}

case_G5_REMOTE_GATE() {
  local wf="$REPO_ROOT/$WF_REL" ci="$REPO_ROOT/$CI_VERIFY_REL" n_wf n_ci

  if [ ! -f "$wf" ]; then
    printf '缺 %s（远端层适配器不存在）\n' "$WF_REL"
    return 1
  fi

  n_wf="$(count_literal "$wf" 'ci-verify.sh')"
  if [ "$n_wf" -eq 0 ]; then
    printf '%s 未调用 %s（闭包第一跳缺失）\n' "$WF_REL" "$CI_VERIFY_REL"
    return 1
  fi

  if [ ! -f "$ci" ]; then
    printf '%s 引用了 %s，但该文件不存在（闭包第二跳断点）\n' "$WF_REL" "$CI_VERIFY_REL"
    return 1
  fi

  n_ci="$(count_literal "$ci" 'verify-note.sh')"
  if [ "$n_ci" -eq 0 ]; then
    printf '%s 未含 verify-note.sh 调用（闭包终点缺失：CI 实际不校验决策记录）\n' "$CI_VERIFY_REL"
    return 1
  fi

  printf '传递闭包成立：%s → %s → verify-note.sh（引用行数 %s / %s）\n' \
    "$WF_REL" "$CI_VERIFY_REL" "$n_wf" "$n_ci"
  return 0
}
