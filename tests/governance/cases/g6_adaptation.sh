#!/usr/bin/env bash
# tests/governance/cases/g6_adaptation.sh — G6-ADAPTATION
#
# 冻结契约：
#   a) 存在 `.agents/notes/**/process/*alt-governance*.md`（适配决策已落盘）；
#   b) `ponygo status` 输出不再含「适配决策未登记」。
# 两者都必须成立：只有文件而 CLI 仍告警 → 落盘位置/命名不合判据；
# 只有文案消失而文件不在 → 文案被改坏（假绿）。
# 额外观测（[INFO]，不参与判定）：候选文件是否被 git 追踪。

case_G6_ADAPTATION() {
  local hits first tracked

  if [ ! -d "$REPO_ROOT/.agents/notes" ]; then
    printf '缺 .agents/notes 目录\n'
    return 1
  fi

  hits="$(find "$REPO_ROOT/.agents/notes" -type f -path '*/process/*alt-governance*.md' 2>/dev/null | sort)"
  if [ -z "$hits" ]; then
    printf '缺 .agents/notes/**/process/*alt-governance*.md（适配决策未落盘）\n'
    return 1
  fi
  first="${hits%%$'\n'*}"
  tracked="$(git -C "$REPO_ROOT" ls-files --error-unmatch "${first#"$REPO_ROOT/"}" >/dev/null 2>&1 && echo tracked || echo untracked)"
  printf '[INFO] 适配决策命中 %s 个文件，首个=%s（git 追踪状态=%s）\n' \
    "$(count_lines "$hits")" "${first#"$REPO_ROOT/"}" "$tracked"

  if ! command -v ponygo >/dev/null 2>&1; then
    printf 'PATH 中找不到 ponygo（PATH=%s），无法验证 status 文案\n' "$PATH"
    return 1
  fi

  run_cmd "$CMD_T" ponygo status
  local rc="$CAP_RC" line
  line="$(printf '%s\n' "$CAP_OUT" | grep -F '适配决策未登记' | head -n1)"
  if [ -n "$line" ]; then
    printf 'ponygo status 仍含「适配决策未登记」（rc=%s）：%s\n' "$rc" "$(oneline "$line")"
    return 1
  fi

  printf '适配决策已落盘（%s）；ponygo status 无「适配决策未登记」（status rc=%s）\n' \
    "${first#"$REPO_ROOT/"}" "$rc"
  return 0
}
