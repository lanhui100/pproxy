#!/usr/bin/env bash
# tests/governance/cases/g12_commitments.sh — G12-COMMITMENTS（Wave-2 追加）
#
# 冻结契约：
#   a) `.meta/gates/instance-commitments.md` 存在；
#   b) 其中含 4 条「承诺 → 命令」行：secret-scan、release-gate、check-egress-parity、check-update-feed；
#   c) alt-governance note 链到该表（承诺表不能是孤儿文件）。
# 语义：每条治理承诺必须落在一条**可执行命令**上；只写承诺不给命令 = 口号，不是门禁。

G12_COMMITMENTS_REL='.meta/gates/instance-commitments.md'
G12_TOKENS='secret-scan release-gate check-egress-parity check-update-feed'

case_G12_COMMITMENTS() {
  local f="$REPO_ROOT/$G12_COMMITMENTS_REL" tok line missing="" found=0 alt alt_rel

  if [ ! -f "$f" ]; then
    printf '缺 %s（承诺→命令映射表不存在）\n' "$G12_COMMITMENTS_REL"
    return 1
  fi

  for tok in $G12_TOKENS; do
    # 命中行（或紧随其后一行）必须给出可执行载体（bash / *.sh / .meta/gates/）才算「承诺→命令」。
    # 容忍 markdown 表格/列表把「命令」折到下一行这类常见写法。
    if awk -v tok="$tok" '
      { lines[NR] = tolower($0) }
      END {
        ok = 0
        for (i = 1; i <= NR; i++) {
          if (index(lines[i], tolower(tok)) > 0) {
            win = lines[i] (i < NR ? " " lines[i + 1] : "")
            if (win ~ /bash/ || win ~ /[.]sh/ || win ~ /gates\//) { ok = 1; break }
          }
        }
        exit(ok ? 0 : 1)
      }' "$f"; then
      found=$((found + 1))
      continue
    fi
    if grep -qF -e "$tok" "$f"; then
      missing="$missing ${tok}(仅承诺无命令)"
    else
      missing="$missing ${tok}(缺失)"
    fi
  done
  if [ -n "$missing" ]; then
    printf '%s 承诺→命令行不全：%s\n' "$G12_COMMITMENTS_REL" "$missing"
    return 1
  fi

  alt="$(find "$REPO_ROOT/.agents/notes" -type f -path '*/process/*alt-governance*.md' 2>/dev/null | sort | head -n1)"
  if [ -z "$alt" ]; then
    printf '缺 .agents/notes/**/process/*alt-governance*.md，无法验证承诺表是否被决策链接\n'
    return 1
  fi
  alt_rel="${alt#"$REPO_ROOT/"}"
  if ! grep -qF -e 'instance-commitments' "$alt"; then
    printf '%s 未链到 %s（承诺表是孤儿文件，读者找不到）\n' "$alt_rel" "$G12_COMMITMENTS_REL"
    return 1
  fi

  printf '四条承诺→命令齐备（%s/%s）；%s 已链接本表\n' \
    "$found" "$(printf '%s' "$G12_TOKENS" | wc -w | tr -d ' ')" "$alt_rel"
  return 0
}
