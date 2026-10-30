#!/usr/bin/env bash
# ============================================================
# adr-verify.sh —— 暂存区 ADR 格式门（门禁资产 A1）
#
# 用法：
#   bash .meta/gates/adr-verify.sh            # 无参 = 校验「暂存区里改动的 ADR」
#   bash .meta/gates/adr-verify.sh <rel>...   # 带参 = 校验指定相对路径（相对仓库根）
#
# 退出码：0 = 放行；非零 = 拒绝（pre-commit 会把非零翻译成"提交被拒"）。
#
# 语义边界（P6 诚实条款）：本门**不自己判** ADR 格式——判定真源是
# `.agents/skills/write-adr/verify-note.sh`（maturity-ladder §5 的 1.1–1.10 机械影子）。
# 本门只多做三件它做不到的事：
#   1) R6 TOCTOU：verify-note 读**工作区**，而 pre-commit 的输入是**暂存区文件名**。
#      两者不一致时（staged 后又在工作区改过），校验的就不是将要提交的那份字节。
#      故先做「暂存区 blob vs 工作区」比对，不一致 → 直接拒绝，要求先 git add。
#   2) R7 覆盖面：用 `--diff-filter=ACMR`（含 R 重命名，`git mv` 不能绕过），
#      并豁免 README.md / AGENTS.md（治理契约载体，非决策记录）。
#   3) F9 非普通文件：symlink（120000）/ gitlink（160000）ADR 一律拒绝——否则"被校验的字节"
#      可以指向工作区外内容，判定面被换掉。此检查必须排在 EOL 归一兜底之前（见 §4a 注释）。
# 为什么不在本门里重写 1.1–1.10：一份判据两份实现必然漂移（P9），
# 且 `ponygo gate adr` 子命令已按同一理由被否决（见 ADR 的 Alternatives R-新子命令）。
#
# 只读纪律：本脚本**不写仓库任何文件**。临时文件一律 `mktemp -d`，`trap` 清理。
# 空场景一律放行（exit 0）：非 git 仓 / 无 .agents/notes / 无暂存 ADR 改动——
# 门禁只对"真的落了 ADR"这一事实负责，不替实例决定要不要写 ADR。
# ============================================================
set -u

PROG=adr-verify
MARK='ADR 有未暂存改动，请先 git add'

note() { printf '[%s] %s\n' "$PROG" "$*"; }
die() {
  printf '[%s] FAIL：%s\n' "$PROG" "$*" >&2
  exit 1
}
skip() {
  printf '[%s] 跳过：%s（exit 0）\n' "$PROG" "$*"
  exit 0
}

# 1) 非 git 仓 → 无暂存区可言，放行（2.4 会另行判"钩子无从挂载"）
ROOT=$(git rev-parse --show-toplevel 2>/dev/null) || skip "非 git 仓库"
[ -n "$ROOT" ] || skip "非 git 仓库"
cd "$ROOT" || die "无法进入仓库根：$ROOT"

# 2) 无 .agents/notes → 该实例没有决策记录制度，本门不越权造制度
[ -d .agents/notes ] || skip "无 .agents/notes 决策记录目录"

# 3) 判定真源必须在位；缺失 = 安装损坏，fail-closed（不得静默放行）
VERIFY=".agents/skills/write-adr/verify-note.sh"
[ -f "$VERIFY" ] || die "缺判定真源 $VERIFY → 恢复该文件后重跑（ponygo init，或 git checkout -- $VERIFY）"

# README.md / AGENTS.md 是治理契约载体，不是决策记录（同 verify-note 1.5/1.6 豁免）
is_adr() {
  case "$1" in
    *.md) ;;
    *) return 1 ;;
  esac
  case "${1##*/}" in
    README.md | AGENTS.md) return 1 ;;
  esac
  return 0
}

PATHS=()
if [ "$#" -gt 0 ]; then
  # 带参模式：只接受 .agents/notes/ 下的相对路径；其余跳过（避免误伤调用者的宽参）
  for p in "$@"; do
    p=${p#./}
    case "$p" in
      .agents/notes/*) ;;
      *)
        note "忽略非 ADR 路径：$p"
        continue
        ;;
    esac
    is_adr "$p" || {
      note "忽略豁免文件：$p"
      continue
    }
    [ -f "$p" ] || die "指定路径不存在：$p（带参校验要求工作区可见该文件）"
    PATHS+=("$p")
  done
else
  # 无参模式：暂存区改动的 ADR（ACMR = 新增/复制/修改/重命名；R7）
  while IFS= read -r p; do
    [ -n "$p" ] || continue
    is_adr "$p" || continue
    PATHS+=("$p")
  done < <(git diff --cached --name-only --diff-filter=ACMR -- '.agents/notes/*.md' 2>/dev/null)
fi

[ "${#PATHS[@]}" -gt 0 ] || skip "无暂存 ADR 变更"

# 4) R6 TOCTOU：暂存字节 vs 工作区字节。不一致 = 校验对象不是将要提交的内容。
TMP=$(mktemp -d) || die "mktemp -d 失败"
trap 'rm -rf "$TMP"' EXIT

drift=0
bad_mode=0
for p in ${PATHS[@]+"${PATHS[@]}"}; do
  if ! git cat-file -e ":$p" 2>/dev/null; then
    printf '[%s] %s：%s（暂存区无该文件；只自查未暂存的新 ADR 请直接跑 bash %s）\n' \
      "$PROG" "$MARK" "$p" "$VERIFY" >&2
    drift=1
    continue
  fi
  git show ":$p" >"$TMP/staged" 2>/dev/null || die "读取暂存区内容失败：$p"
  # 4a) F9：ADR 必须是普通文件（symlink 120000 / gitlink 160000 一律拒绝）。
  # 位置是判据本身的部分：必须早于 cmp 与下面的 EOL 归一兜底——
  # symlink 暂存时 blob = 链接文本、cmp 读工作区 = 目标内容，两者必然不同，
  # 而 `git diff --quiet` 对 symlink 只说"链接自身没改"（无差异）→ 会被兜底当假阳性放过。
  # 实测根因即此，故模式检查排在兜底之前。
  mode=$(git ls-files -s -- "$p" 2>/dev/null | awk '{print $1}' | head -1)
  case "$mode" in
    120000 | 160000)
      printf '[%s] 拒绝：%s 不是普通文件（mode=%s；ADR 必须是普通文件）\n' "$PROG" "$p" "$mode" >&2
      bad_mode=1
      continue
      ;;
  esac
  if cmp -s "$TMP/staged" "$p"; then
    continue
  fi
  # 字节不同还要排除一种假阳性：属性层 EOL 归一（.gitattributes `* text=auto eol=lf`）。
  # `git diff --quiet` 按 clean filter 比较索引与工作区——它说"无差异"，
  # 说明差异只来自归一化而非未暂存编辑。
  if git diff --quiet -- "$p" 2>/dev/null; then
    continue
  fi
  printf '[%s] %s：%s\n' "$PROG" "$MARK" "$p" >&2
  drift=1
done

if [ "$bad_mode" -ne 0 ] || [ "$drift" -ne 0 ]; then
  # 按因给补救（不给一条万能命令）：两类成因的修法完全不同。
  if [ "$bad_mode" -ne 0 ]; then
    printf '[%s] 拒绝：ADR 必须是普通文件（符号链接/gitlink 会让被校验的字节指向工作区外）→ 用普通文件重写该 ADR 后 git add\n' "$PROG" >&2
  fi
  if [ "$drift" -ne 0 ]; then
    printf '[%s] 拒绝：暂存区与工作区不一致（verify-note 读工作区，会校验错字节）→ 先 git add 再提交\n' "$PROG" >&2
  fi
  exit 1
fi

# 5) 交给判定真源（唯一真值，不在本门复刻 1.1–1.10）
bash "$VERIFY" ${PATHS[@]+"${PATHS[@]}"} || exit 1

printf '[%s] 通过：%d 条 ADR 经 %s 校验（暂存区与工作区一致）\n' "$PROG" "${#PATHS[@]}" "$VERIFY"
exit 0
