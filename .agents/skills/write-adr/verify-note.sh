#!/usr/bin/env bash
# ============================================================
# verify-note.sh —— 决策记录机械校验器（write-adr 技能的验证脚本）
#
# 用法：bash .agents/skills/write-adr/verify-note.sh [<notes 相对路径>…]
#   无参     = 校验整个 .agents/notes/ 树（推荐；等价 ponygo status 的 L1 判据）
#   带路径   = 只校验给定文件（如 .agents/notes/proposed/architecture/xxx.md）
#
# 退出码：全部通过 exit 0；任一违例 exit 1（逐项打印 FAIL）。
# 对应判据：ponygo verify_ladder 1.1–1.9（maturity-ladder §5），
# 影子来自 DSH verify-agent-note-format / verify-agent-note-classification。
#
# 设计快照（为何是 bash、为何一个脚本）：
# - 语言：bash 与 ponygo CLI 同源（单文件、零依赖铁律，见 notes/
#   implemented/architecture/2026-08-30-single-file-zero-dependency-cli.md）。
#   ponygo 是栈无关元框架，实例可能是任意语言——只有 bash 是装了 ponygo 就
#   保证存在的运行时。DSH 用 TS 是因为其本身就是 Node monorepo，工具链现成；
#   两种定位的正确分岔，非能力差异。
# - 合并一脚本：DSH 拆 format/classification 两个 gate 是服务其门禁矩阵的粒度
#   （分开挂钩子/CI、独立 spec）；ponygo 的真源合一（maturity-ladder §5 一张表），
#   且本脚本的消费者是 write-adr 的"写一条验证一次"自证步——单出口优于双出口。
#   未来实例升 L2 建门禁矩阵时再拆分，YAGNI。
#
# 语义边界（P6 诚实条款）：本脚本只锁**机械可查的影子**——路径两轴、文件名、
# 日期合法性、Status 与目录一致、骨架标题与 lifecycle 匹配。
# 不判"implemented 内容是否真的已落地"（真实性）与"多类是否该拆条"
# （分类）——这两者是语义判断，靠 review + write-adr 校准样例。
#
# 对齐契约：本脚本与 CLI 的 verify_ladder 1.1–1.9 逐项对齐；若发现行为不一致，
# 以 maturity-ladder §5 判据真源为准并回改两者。
# ============================================================
set -u

NOTES_DIR=".agents/notes"
LIFECYCLES="proposed implemented rejected archived"
CLASSES="feature bug-fix simplification architecture process testing"
FAIL=0
fail() { printf 'FAIL  %s\n' "$*" >&2; FAIL=1; }

[ -d "$NOTES_DIR" ] || { echo "verify-note: 无 $NOTES_DIR（先 ponygo init）" >&2; exit 1; }

# 实例扩展 class（classes.local，与 CLI extra_classes 同构）
extra_classes() {
  [ -f "$NOTES_DIR/classes.local" ] || return 0
  sed 's/\r$//' "$NOTES_DIR/classes.local" | grep -vE '^[[:space:]]*(#|$)' | grep -E '^[a-z0-9][a-z0-9-]*$'
}

# 1.3 顶层目录 ⊆ 四态封闭集
while IFS= read -r d; do
  case " $LIFECYCLES " in *" $d "*) ;; *) fail "1.3 顶层目录越界：$d" ;; esac
done < <(find "$NOTES_DIR" -mindepth 1 -maxdepth 1 -type d -printf '%f\n' 2>/dev/null)

# 1.4 二级目录 ⊆ 六类封闭集 ∪ classes.local
classes_all=" $CLASSES $(extra_classes | tr '\n' ' ') "
while IFS= read -r d; do
  case "$classes_all" in *" $d "*) ;; *) fail "1.4 class 越界：$d" ;; esac
done < <(find "$NOTES_DIR" -mindepth 2 -maxdepth 2 -type d -printf '%f\n' 2>/dev/null)

# DATE_OK：GNU date 可用才机械判日期，否则降级"靠 review"（与 CLI 同款诚实条款）
DATE_OK=0
date -d @0 '+%Y' >/dev/null 2>&1 && DATE_OK=1

check_one() { # $1 = 决策文件路径
  local f="$1" b rel lc slashes st d8
  b="${f##*/}"
  [ "$b" = "README.md" ] || [ "$b" = "AGENTS.md" ] && return 0
  rel="${f#"$NOTES_DIR"/}"
  [ "$rel" = "$f" ] && rel="$f"
  lc="${rel%%/*}"
  # 1.5 深度：恰好 {lifecycle}/{class}/ 两级
  slashes="${rel//[!\/]/}"
  [ "${#slashes}" -eq 2 ] || { fail "1.5 $f：未恰好位于 {lifecycle}/{class}/ 两级"; return 0; }
  # 1.6 文件名 + 日期合法性
  if ! printf '%s\n' "$b" | grep -qE '^[0-9]{4}-[0-9]{2}-[0-9]{2}-.+\.md$'; then
    fail "1.6 $f：文件名不符 yyyy-mm-dd-topic.md"
    return 0
  fi
  if [ "$DATE_OK" = "1" ]; then
    d8="${b:0:10}"
    date -d "$d8" >/dev/null 2>&1 || fail "1.6 $f：日期不合法（$d8）"
  fi
  # 1.7 Status 首词与所在 lifecycle 一致（archived 允许 implemented/archived）
  st=$(sed -n 's/\r$//;s/^Status:[[:space:]]*//p' "$f" 2>/dev/null | head -n1 | awk '{print $1}')
  if [ -z "$st" ]; then
    fail "1.7 $f：缺 Status: 行"
  elif [ "$lc" = "archived" ]; then
    [ "$st" = "implemented" ] || [ "$st" = "archived" ] || fail "1.7 $f：archived 下 Status 应为 implemented/archived，实得 $st"
  elif [ "$st" != "$lc" ]; then
    fail "1.7 $f：Status($st) 与目录($lc) 不一致"
  fi
  # 1.8 骨架标题与 lifecycle 匹配
  case "$lc" in
    implemented)
      grep -qE '^## (Proposal|Plan|Migration plan|Acceptance criteria)([[:space:]]|$)' "$f" \
        && fail "1.8 $f：implemented 含提案时代标题（## Proposal/## Plan/## Migration plan/## Acceptance criteria）" ;;
    proposed|rejected)
      grep -qE '^## Proposal([[:space:]]|$)' "$f" || fail "1.8 $f：$lc 缺 ## Proposal"
      grep -qE '^## Decision([[:space:]]|$)' "$f" && fail "1.8 $f：$lc 含现在时 ## Decision（提案伪装成决定）" ;;
  esac
  return 0
}

if [ $# -gt 0 ]; then
  for f in "$@"; do check_one "$f"; done
else
  while IFS= read -r -d '' f; do check_one "$f"; done \
    < <(find "$NOTES_DIR" -type f -name '*.md' -print0 2>/dev/null)
  # 1.9 文档有家（v2.0，整树模式才判）：docs/AGENTS.md 是文档标准的家（agent 自动加载）。
  # 与 1.1-1.8 同属 L1 判据；docs/AGENTS.md 由 ponygo init 骨架播种。
  [ -f "docs/AGENTS.md" ] || fail "1.9 缺文档标准家 docs/AGENTS.md（agent 自动加载）→ 补 docs/AGENTS.md（tier 分类 + 写作规则 + slop checklist）"
fi

if [ "$FAIL" = "0" ]; then
  echo "verify-note: 全部通过（机械影子 1.1–1.9；真实性/分类靠 review）"
  exit 0
fi
echo "verify-note: 未通过（见上方 FAIL 项；逐项修复后重跑）" >&2
exit 1
