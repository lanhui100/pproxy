#!/usr/bin/env bash
# tests/governance/cases/g4_sentinel_local.sh — G4-SENTINEL-LOCAL
#
# 冻结契约：
#   a) `.githooks/pre-commit` 同时含 `>>> ponygo-gate: adr-verify v1 >>>` 与 `<<< ponygo-gate: adr-verify v1 <<<`；
#   b) `.meta/gates/adr-verify.sh` 存在；
#   c) `ponygo status` exit 0（脚本所在 PATH 内有 ponygo）。
# 补充的一条最小结构语义（"同时含"的自然含义）：起始哨兵行号必须早于结束哨兵行号，
#   否则哨兵块是坏的（门禁块无法被识别），不能算"已接线"。

SENTINEL_BEGIN='>>> ponygo-gate: adr-verify v1 >>>'
SENTINEL_END='<<< ponygo-gate: adr-verify v1 <<<'
PRE_COMMIT_REL='.githooks/pre-commit'
ADR_VERIFY_REL='.meta/gates/adr-verify.sh'

# probe_sentinel_pair [<file>] — 0=合规（两哨兵齐备且有序）
probe_sentinel_pair() {
  local f="${1:-$REPO_ROOT/$PRE_COMMIT_REL}" b e
  [ -f "$f" ] || {
    printf '缺哨兵宿主文件 %s\n' "$f"
    return 1
  }
  b="$(grep -nF -e "$SENTINEL_BEGIN" "$f" 2>/dev/null | head -n1 | cut -d: -f1)"
  e="$(grep -nF -e "$SENTINEL_END" "$f" 2>/dev/null | head -n1 | cut -d: -f1)"
  if [ -z "$b" ] || [ -z "$e" ]; then
    printf '%s 哨兵缺失：begin=%s end=%s（期望两行都在）\n' \
      "$f" "${b:-缺}" "${e:-缺}"
    return 1
  fi
  if [ "$b" -ge "$e" ]; then
    printf '%s 哨兵顺序错：begin@%s 应在 end@%s 之前\n' "$f" "$b" "$e"
    return 1
  fi
  printf '%s 哨兵配对完整（begin@%s → end@%s）\n' "$f" "$b" "$e"
  return 0
}

case_G4_SENTINEL_LOCAL() {
  probe_sentinel_pair "$REPO_ROOT/$PRE_COMMIT_REL" || return 1

  if [ ! -f "$REPO_ROOT/$ADR_VERIFY_REL" ]; then
    printf '缺 %s（哨兵块指向的本地门禁脚本不存在，接线是空壳）\n' "$ADR_VERIFY_REL"
    return 1
  fi

  if ! command -v ponygo >/dev/null 2>&1; then
    printf 'PATH 中找不到 ponygo（PATH=%s），无法验证 status 自洽\n' "$PATH"
    return 1
  fi

  run_cmd "$CMD_T" ponygo status
  local rc="$CAP_RC" key
  key="$(printf '%s\n' "$CAP_OUT" | grep -E '治理级|未接线|级自洽验证|适配决策未登记' | head -n2 | tr '\n' ' ')"
  if [ "$rc" -ne 0 ]; then
    printf 'ponygo status 应 exit 0（治理级自洽），实得 rc=%s；关键行：%s\n' "$rc" "$(oneline "$key")"
    return 1
  fi
  printf '哨兵配对（%s）+ %s 存在 + ponygo status rc=0（关键行：%s）\n' \
    "$PRE_COMMIT_REL" "$ADR_VERIFY_REL" "$(oneline "$key")"
  return 0
}
