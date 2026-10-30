#!/usr/bin/env bash
# tests/governance/cases/g16_observe_decision.sh — G16-OBSERVE-DECISION（Wave-2 追加）
#
# 冻结契约：
#   a) 存在 `.agents/notes/implemented/process/*observe*` note；
#   b) 该 note 明确「可观测性四维暂不接」+ 理由 + 重估触发条件；
#   c) `ponygo observe status` 仍为缺失（不强制 init）。
# 语义：「暂不接」是一个**决策**，不是遗漏 —— 必须有记录、有理由、有重估条件；
#   同时不得反向偷偷 init（那样 (b) 就变成假话 → (c) 守住这个边界）。
# 说明：(b) 是文档语义判据，只能做关键词组判定；各组缺失即 FAIL 并回报实测命中情况（fail-closed）。

G16_OBSERVE_DIR='.agents/notes/implemented/process'

# 关键词组：用 | 分隔同义写法
G16_KW_DIMENSION='可观测性'
G16_KW_DEFER='暂不接|不接入|暂缓接入|不在本轮接入|维持缺口'
G16_KW_REASON='理由|因由|原因|动机|Rationale|rationale'
G16_KW_REVISIT='重估|触发条件|触发时机|Revisit|revisit|回访'

case_G16_OBSERVE_DECISION() {
  local dir="$REPO_ROOT/$G16_OBSERVE_DIR" note note_rel body

  if [ ! -d "$dir" ]; then
    printf '缺目录 %s（无 process 类决策落点）\n' "$G16_OBSERVE_DIR"
    return 1
  fi
  note="$(find "$dir" -maxdepth 1 -type f -iname '*observe*' 2>/dev/null | sort | head -n1)"
  if [ -z "$note" ]; then
    printf '缺 %s/*observe* 决策 note（可观测性四维接不接没有记录）\n' "$G16_OBSERVE_DIR"
    return 1
  fi
  note_rel="${note#"$REPO_ROOT/"}"
  body="$(cat "$note")"

  if ! printf '%s\n' "$body" | grep -qE "$G16_KW_DIMENSION"; then
    printf '%s 未提及「%s」（判据对象缺失）\n' "$note_rel" "$G16_KW_DIMENSION"
    return 1
  fi
  if ! printf '%s\n' "$body" | grep -qE "$G16_KW_DEFER"; then
    printf '%s 未明确「四维暂不接」的结论（命中 0：需含 %s 之一）\n' "$note_rel" "$G16_KW_DEFER"
    return 1
  fi
  if ! printf '%s\n' "$body" | grep -qE "$G16_KW_REASON"; then
    printf '%s 缺「理由」（命中 0：需含 %s 之一）\n' "$note_rel" "$G16_KW_REASON"
    return 1
  fi
  if ! printf '%s\n' "$body" | grep -qE "$G16_KW_REVISIT"; then
    printf '%s 缺「重估触发条件」（命中 0：需含 %s 之一）\n' "$note_rel" "$G16_KW_REVISIT"
    return 1
  fi

  # (c) 不强制 init：声明必须仍为缺失
  if ! command -v ponygo >/dev/null 2>&1; then
    printf 'PATH 中找不到 ponygo（PATH=%s），无法验证 observe 声明仍缺失\n' "$PATH"
    return 1
  fi
  run_cmd "$CMD_T" ponygo observe status
  if [ "$CAP_RC" -ne 0 ]; then
    printf 'ponygo observe status 异常（rc=%s）：%s\n' "$CAP_RC" "$(oneline "$CAP_OUT")"
    return 1
  fi
  if ! printf '%s\n' "$CAP_OUT" | grep -qE '声明:[[:space:]]*缺失|declaration_missing'; then
    printf 'ponygo observe status 显示声明已存在（与「暂不接」决策冲突，不得偷偷 init）：%s\n' \
      "$(printf '%s\n' "$CAP_OUT" | grep -E '声明:|declaration_' | head -n2 | tr '\n' ' ')"
    return 1
  fi

  printf '决策 note=%s 含「四维暂不接」+理由+重估触发；ponygo observe status 声明仍缺失（rc=0）\n' "$note_rel"
  return 0
}
