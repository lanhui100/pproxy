#!/usr/bin/env bash
# tests/governance/cases/g10_supersession.sh — G10-SUPERSESSION（Wave-2 追加）
#
# 冻结契约：
#   a) `.agents/notes/implemented/architecture/2026-10-07-desktop-toast-borderless-frosted.md`
#      含 `Supersedes:` 且指向 2026-09-26 / 2026-09-27 两条旧 note；
#   b) 两条旧 note 各含一行「被 …2026-10-07… 取代」。
# 语义：决策链必须双向可追（新声明"我取代谁"，旧声明"我被谁取代"）。
#   单向即断链：只在新 note 写 Supersedes，读者顺着旧 note 走会以为它仍然有效。

G10_NEW_NOTE='.agents/notes/implemented/architecture/2026-10-07-desktop-toast-borderless-frosted.md'
G10_OLD_NOTES='.agents/notes/implemented/architecture/2026-09-26-desktop-toast-style-redesign.md .agents/notes/implemented/architecture/2026-09-27-desktop-toast-light-gray-redesign.md'
G10_SUPERSEDED_DATES='2026-09-26 2026-09-27'
G10_SUPERSEDING_DATE='2026-10-07'

case_G10_SUPERSESSION() {
  local newf="$REPO_ROOT/$G10_NEW_NOTE" sup d missing="" old hit n_old=0

  if [ ! -f "$newf" ]; then
    printf '缺新决策 %s（取代关系无处声明）\n' "$G10_NEW_NOTE"
    return 1
  fi

  # 容错：允许 `Supersedes:` / `- Supersedes:` / `**Supersedes**:` / frontmatter `supersedes:` 等书写
  sup="$(grep -niE 'supersedes' "$newf" || true)"
  if [ -z "$sup" ]; then
    printf '%s 缺 Supersedes 字段（未声明它取代了哪些旧决策）\n' "$G10_NEW_NOTE"
    return 1
  fi
  for d in $G10_SUPERSEDED_DATES; do
    printf '%s\n' "$sup" | grep -qF -e "$d" || missing="$missing $d"
  done
  if [ -n "$missing" ]; then
    printf '%s 的 Supersedes 未指向：%s（实得：%s）\n' "$G10_NEW_NOTE" "$missing" "$(oneline "$sup")"
    return 1
  fi

  for old in $G10_OLD_NOTES; do
    if [ ! -f "$REPO_ROOT/$old" ]; then
      printf '缺被取代的旧决策 %s（Supersedes 指向了不存在的文件）\n' "$old"
      return 1
    fi
    # 同一行既要出现取代者日期、又要出现「取代」语义（顺序不限，但必须同行成句）
    hit="$(grep -nF -e "$G10_SUPERSEDING_DATE" "$REPO_ROOT/$old" | grep -F -e '取代' || true)"
    if [ -z "$hit" ]; then
      printf '%s 缺「被 …%s… 取代」行（旧决策无回指，链断一半）\n' "$old" "$G10_SUPERSEDING_DATE"
      return 1
    fi
    n_old=$((n_old + 1))
  done

  printf 'Supersedes 指向 %s；两条旧 note 各含取代行（%s 条）\n' \
    "$(printf '%s' "$G10_SUPERSEDED_DATES" | tr ' ' '+')" "$n_old"
  return 0
}
