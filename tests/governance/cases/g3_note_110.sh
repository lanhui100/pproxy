#!/usr/bin/env bash
# tests/governance/cases/g3_note_110.sh — G3-NOTE-1.10
#
# 冻结契约：构造临时 note 缺 `## Alternatives considered`，
#          `bash .agents/skills/write-adr/verify-note.sh <tmp>` 必须 exit 非零；合法 note exit 0。
# 矩阵修订（Lead）：临时 note 必须放仓库外（$TMPROOT），并显式传**相对路径**参数。
#   - verify-note.sh 的带参模式只校验给定文件（不判 1.9 文档有家）；
#     若不加参数即为整树模式，会额外判 1.9 并扫描仓库内全部决策记录 → 与本用例目标不符。
#   - 带参模式下脚本仍会校验 cwd 的 `.agents/notes` 一/二级目录封闭集，
#     故影子树必须建出 .agents/notes/{lifecycle}/{class}/ 的合法骨架。
#
# 退出码映射：带参调用 rc=0 表示放行，非零表示拒绝（含超时 124 → 拒）。

VERIFIER_REL='.agents/skills/write-adr/verify-note.sh'
G3_BAD_REL='.agents/notes/proposed/process/2026-10-30-g3-missing-alternatives.md'
G3_GOOD_REL='.agents/notes/proposed/process/2026-10-30-g3-compliant.md'

# probe_note_section_enforced <verifier-abs> <root> <bad-rel> <good-rel>
#   0 = 合规（bad 被拒 且 good 放行）；非零 = 违规
probe_note_section_enforced() {
  local verifier="$1" root="$2" bad_rel="$3" good_rel="$4" rc_bad rc_good bad_out good_out

  [ -f "$verifier" ] || {
    printf '缺决策记录校验器 %s\n' "$verifier"
    return 1
  }
  [ -d "$root/.agents/notes" ] || {
    printf '影子树缺 .agents/notes（root=%s）\n' "$root"
    return 1
  }
  [ -f "$root/$bad_rel" ] || {
    printf '缺反例夹具 %s\n' "$root/$bad_rel"
    return 1
  }
  [ -f "$root/$good_rel" ] || {
    printf '缺正例夹具 %s\n' "$root/$good_rel"
    return 1
  }

  # 反例：缺 `## Alternatives considered` → 必须 exit 非零
  run_cmd_in "$root" "$CMD_T" bash "$verifier" "$bad_rel"
  rc_bad="$CAP_RC"
  bad_out="$(last_line "$CAP_OUT")"

  # 正例：合法 note → 必须 exit 0（防"恒拒绝"型假红）
  run_cmd_in "$root" "$CMD_T" bash "$verifier" "$good_rel"
  rc_good="$CAP_RC"
  good_out="$(last_line "$CAP_OUT")"

  if [ "$rc_bad" -eq 0 ]; then
    printf '缺「## Alternatives considered」的 note 未被拒（rc=0，1.10 判据缺失）；校验器输出：%s\n' \
      "$bad_out"
    return 1
  fi
  if [ "$rc_good" -ne 0 ]; then
    printf '合法 note 被误拒（rc=%s，假红）；校验器输出：%s\n' "$rc_good" "$good_out"
    return 1
  fi
  printf '1.10 生效：缺节 note rc=%s（被拒）；合法 note rc=0（放行）\n' "$rc_bad"
  return 0
}

case_G3_NOTE_110() {
  local root="$TMPROOT/g3"
  rm -rf "$root"
  mkdir -p "$root/.agents/notes/proposed/process" || {
    printf '无法创建影子决策记录树 %s\n' "$root"
    return 1
  }

  printf '# Agent Note: G3 反例（缺 Alternatives considered）\nStatus: proposed\n\n## Proposal\n仅提案，故意省略「## Alternatives considered」一节。\n' \
    >"$root/$G3_BAD_REL"
  printf '# Agent Note: G3 正例（合法）\nStatus: proposed\n\n## Proposal\n提案正文。\n\n## Alternatives considered\n备选方案与被否原因。\n' \
    >"$root/$G3_GOOD_REL"

  probe_note_section_enforced "$REPO_ROOT/$VERIFIER_REL" "$root" "$G3_BAD_REL" "$G3_GOOD_REL"
}
