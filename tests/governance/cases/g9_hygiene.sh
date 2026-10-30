#!/usr/bin/env bash
# tests/governance/cases/g9_hygiene.sh — G9-HYGIENE
#
# 冻结契约：`git ls-files '.agents/tests/**' | grep -E '\.(log|txt|csv)$'` 为空
#          （PLAN.md 除外可豁免 —— 即使它本不匹配上述后缀，也显式排除，避免契约措辞歧义）。
# 语义：测试过程产物（日志/文本/CSV）不得进入版本库；缺目录/0 文件视为合规。
#
# 退出码映射：git ls-files 失败（非 0）→ 无法判定 → FAIL（fail-closed）。

G9_PATHS_SPEC='.agents/tests/**'

case_G9_HYGIENE() {
  local total bad nbad

  run_cmd "$CMD_T" git -C "$REPO_ROOT" ls-files "$G9_PATHS_SPEC"
  if [ "$CAP_RC" -ne 0 ]; then
    printf 'git ls-files %s 执行失败（rc=%s）：%s\n' \
      "$G9_PATHS_SPEC" "$CAP_RC" "$(oneline "$CAP_OUT")"
    return 1
  fi

  total="$(count_lines "$CAP_OUT")"
  if [ "$total" -eq 0 ]; then
    printf 'git ls-files %s 0 个文件（无过程产物入库）\n' "$G9_PATHS_SPEC"
    return 0
  fi

  # 契约字面过滤 + PLAN.md 豁免
  bad="$(printf '%s\n' "$CAP_OUT" | grep -E '\.(log|txt|csv)$' || true)"
  bad="$(printf '%s\n' "$bad" | grep -vE '(^|/)PLAN\.md$' || true)"
  nbad="$(count_lines "$bad")"

  if [ "$nbad" -ne 0 ]; then
    printf '追踪的测试产物 %s 个（期望 0；豁免 PLAN.md）：%s\n' "$nbad" "$(list_head "$bad" 5)"
    return 1
  fi

  printf '.agents/tests 追踪文件 %s 个，其中 .log/.txt/.csv 产物 0 个（豁免 PLAN.md）\n' "$total"
  return 0
}
