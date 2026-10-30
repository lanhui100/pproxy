#!/usr/bin/env bash
# tests/governance/cases/g11_roadmap_index.sh — G11-ROADMAP-INDEX（Wave-2 追加）
#
# 冻结契约：
#   a) `README.md` 的文档索引行含 `M1-M7`（**不再是** `M1-M6`）；
#   b) `docs/product/ROADMAP.md` 含 `## M7`。
# 语义：索引与真源一致。ROADMAP 已开 M7 而索引还停在 M1-M6 → 索引把读者引到旧视野（文档漂移）。

G11_ROADMAP_REL='docs/product/ROADMAP.md'

case_G11_ROADMAP_INDEX() {
  local readme="$REPO_ROOT/README.md"
  local road="$REPO_ROOT/$G11_ROADMAP_REL"
  local idx n7 n6 m6_lines

  if [ ! -f "$readme" ]; then
    printf '缺仓库根 README.md\n'
    return 1
  fi

  idx="$(grep -nE 'M1-M[0-9]+' "$readme" || true)"
  if [ -z "$idx" ]; then
    printf 'README.md 未见 M1-M<N> 文档索引行（索引无法核对里程碑区间）\n'
    return 1
  fi

  n7="$(printf '%s\n' "$idx" | grep -c 'M1-M7' || true)"
  n6="$(printf '%s\n' "$idx" | grep -c 'M1-M6' || true)"
  if [ "${n7:-0}" -lt 1 ]; then
    printf 'README.md 索引行未升级到 M1-M7：%s\n' "$(list_head "$idx" 3)"
    return 1
  fi
  if [ "${n6:-0}" -ne 0 ]; then
    m6_lines="$(printf '%s\n' "$idx" | grep 'M1-M6' || true)"
    printf 'README.md 索引行仍残留 M1-M6（%s 处）：%s\n' "${n6}" "$(list_head "$m6_lines" 3)"
    return 1
  fi

  if [ ! -f "$road" ]; then
    printf '缺 %s（里程碑真源不存在）\n' "$G11_ROADMAP_REL"
    return 1
  fi
  if ! grep -qE '^## M7' "$road"; then
    printf '%s 缺 `## M7` 里程碑段\n' "$G11_ROADMAP_REL"
    return 1
  fi

  printf 'README 索引 %s 行含 M1-M7 且无 M1-M6；%s 含 ## M7\n' "${n7}" "$G11_ROADMAP_REL"
  return 0
}
