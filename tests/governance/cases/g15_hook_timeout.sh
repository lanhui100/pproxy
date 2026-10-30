#!/usr/bin/env bash
# tests/governance/cases/g15_hook_timeout.sh — G15-HOOK-TIMEOUT（Wave-2 追加）
#
# 冻结契约：`.githooks/pre-commit` 中 `cargo check` 被 `timeout` 包裹（`grep -q 'timeout'`）。
# 语义：pre-commit 是阻塞式本地门禁；`cargo check --workspace` 在冷缓存/锁争用下可能数分钟甚至挂死，
#   无超时保护会把开发者卡在提交环节（并诱发 `--no-verify` 绕过 → 门禁形同不存在）。

G15_HOOK_REL='.githooks/pre-commit'

case_G15_HOOK_TIMEOUT() {
  local f="$REPO_ROOT/$G15_HOOK_REL" n_cargo=0 wrapped=0 line_no cur prev tmo_n fallback=0

  if [ ! -f "$f" ]; then
    printf '缺 %s\n' "$G15_HOOK_REL"
    return 1
  fi

  # 契约字面：钩子内必须出现 timeout
  if ! grep -q 'timeout' "$f"; then
    printf '%s 未出现 timeout（cargo check 无超时保护，钩子可无限挂起）\n' "$G15_HOOK_REL"
    return 1
  fi
  tmo_n="$(grep -c 'timeout' "$f" || true)"

  # 语义：timeout 必须落在 cargo check 上（同行 / 上一行续行 / 包装变量或函数名）
  while IFS= read -r line_no; do
    [ -n "$line_no" ] || continue
    n_cargo=$((n_cargo + 1))
    cur="$(sed -n "${line_no}p" "$f")"
    prev="$(sed -n "$((line_no - 1))p" "$f")"
    if printf '%s\n' "$cur" | grep -q 'timeout' || printf '%s\n' "$prev" | grep -q 'timeout'; then
      wrapped=$((wrapped + 1))
    fi
  done < <(grep -n 'cargo check' "$f" | cut -d: -f1)

  if [ "$n_cargo" -eq 0 ]; then
    printf '%s 未包含 cargo check（契约以"包裹 cargo check"为前提，缺失即无法判定）\n' "$G15_HOOK_REL"
    return 1
  fi

  # 兜底判据：任意位置出现「timeout … cargo」形式（含 `timeout N cargo check` / 变量化写法）
  if grep -qE 'timeout[^#]*cargo' "$f"; then
    fallback=1
  fi
  if [ "$wrapped" -eq 0 ] && [ "$fallback" -eq 0 ]; then
    printf '%s 有 timeout（%s 处）但 cargo check 未被其包裹（%s 处 cargo check 均为裸调用；实得：%s）\n' \
      "$G15_HOOK_REL" "$tmo_n" "$n_cargo" "$(grep -n 'cargo check' "$f" | head -n2 | tr '\n' ' ')"
    return 1
  fi

  if [ "$wrapped" -eq 0 ]; then
    printf '%s 的 cargo check 由「timeout … cargo」形式包裹（%s 处 cargo check，timeout 出现 %s 处）\n' \
      "$G15_HOOK_REL" "$n_cargo" "$tmo_n"
  else
    printf '%s 的 %s 处 cargo check 中有 %s 处被 timeout 包裹（timeout 出现 %s 处）\n' \
      "$G15_HOOK_REL" "$n_cargo" "$wrapped" "$tmo_n"
  fi
  return 0
}
