#!/usr/bin/env bash
# tests/governance/cases/g7_implemented_reproducible.sh — G7-IMPLEMENTED-REPRODUCIBLE
#
# 冻结契约（Lead 修订：基线已前移到 b7f1387）：
#   a) `git log -S'absolute_form' -- crates/server/src/gateway.rs` 非空（决策有可回放提交）；
#   b) `git show HEAD:crates/server/src/gateway.rs | grep -q forward_proxy_unsupported`（实现确已落地）。
# 二者缺一不可：只有提交而无实现 = "记了没做"；只有实现而无提交 = "做了没记录（不可回放）"。

G7_PATH='crates/server/src/gateway.rs'

case_G7_IMPLEMENTED_REPRODUCIBLE() {
  local n_commit show_rc n_impl log_out newest

  run_cmd "$CMD_T" git -C "$REPO_ROOT" log --oneline -S'absolute_form' -- "$G7_PATH"
  if [ "$CAP_RC" -ne 0 ]; then
    printf 'git log -S"absolute_form" -- %s 执行失败（rc=%s）：%s\n' \
      "$G7_PATH" "$CAP_RC" "$(oneline "$CAP_OUT")"
    return 1
  fi
  log_out="$CAP_OUT"
  n_commit="$(count_lines "$log_out")"
  if [ "$n_commit" -eq 0 ]; then
    printf 'git log -S"absolute_form" -- %s 为空（决策无可回放提交）\n' "$G7_PATH"
    return 1
  fi
  newest="$(printf '%s\n' "$log_out" | awk 'NF' | head -n1 | cut -c1-80)"

  run_cmd "$CMD_T" git -C "$REPO_ROOT" show "HEAD:$G7_PATH"
  show_rc="$CAP_RC"
  if [ "$show_rc" -ne 0 ]; then
    printf 'git show HEAD:%s 失败（rc=%s）：%s\n' "$G7_PATH" "$show_rc" "$(oneline "$CAP_OUT")"
    return 1
  fi
  # grep -c 走 here-string（非管道）→ 不受 SIGPIPE/pipefail 干扰，rc 与命中数都可信
  n_impl="$(grep -c -e 'forward_proxy_unsupported' <<<"$CAP_OUT" || true)"
  if [ "${n_impl:-0}" -eq 0 ]; then
    printf 'HEAD 的 %s 不含 forward_proxy_unsupported（实现未落地）\n' "$G7_PATH"
    return 1
  fi

  printf 'git log -S 命中 %s 个提交（最新 %s）；HEAD:%s 含 forward_proxy_unsupported %s 处\n' \
    "$n_commit" "$newest" "$G7_PATH" "$n_impl"
  return 0
}
