#!/usr/bin/env bash
# ============================================================
# archive-seal.sh —— 归档封印门（archived/ 一旦封印，内容不得静默改写）
#
# 用法：
#   bash .meta/gates/archive-seal.sh --check              # 校验封印（默认语义；CI/pre-push 用）
#   bash .meta/gates/archive-seal.sh --write              # 重新封印（**只在归档动作的同一提交里**执行）
#   bash .meta/gates/archive-seal.sh --check --root DIR   # 对 DIR 这个检出/副本校验（负样本 spec 用）
#
# 退出码：
#   0 = 一致（放行）
#   1 = 不一致（篡改 / 缺失 / 未封印的新文件）
#   2 = 环境或用法错误（缺 manifest、目录不存在、sha256sum 缺失）——fail-closed，不算放行
#
# 为什么需要它：`archived/` 是**曾经落地、后被取代**的历史快照（见 `.agents/notes/README.md`）。
#   "归档即冻结"如果没有机械看守，就不是承诺而是礼貌——任何人（包括未来的自己）都能顺手改一行
#   让历史与当下对不上，而读者无从发现。封印 = 把"不该变"变成一条会非零退出的命令。
#
# 判据形态：`archived/` 下每个文件的 sha256 清单落 `.agents/notes/archived/SHA256SUMS`。
#   `--check` 做三件事：①清单内每个文件的哈希是否仍一致；②清单内文件是否还在（缺失=1）；
#   ③目录里是否有清单外的文件（新增/未封印=1，防"绕过封印偷偷塞条目")。
#   `--write` 只做一件事：按当前内容重建清单（含新增、剔除已删）。
#
# 边界（P6 诚实条款）：
#   - 只覆盖 `archived/` 一个目录；其余 lifecycle 的"不该变"由 supersession 与 review 承担；
#   - 只证明"内容与封印时一致"，**不证明封印时内容是对的**（那是 review 与归档动作的时点判断）；
#   - 封印不防恶意改写者（他能同时改清单）——它防的是**无意的漂移**：改了就红，红了就得解释。
#
# 依赖：find / sort / sha256sum / awk / sed（POSIX 工具集），不依赖 git、不联网。
# 只读纪律（准确口径，别写"不写任何文件"——那是假话）：
#   - `--write` 只写 `<root>/.agents/notes/archived/SHA256SUMS`（唯一产物）；
#   - `--check` **不改动仓库内任何文件**，但会在 `$TMPDIR` 建临时清单文件（`trap` 清理）。
#   - 两个模式都不写仓库外除 `$TMPDIR` 之外的位置。
# ============================================================
set -u

PROG=archive-seal
ARCHIVED_REL='.agents/notes/archived'
MANIFEST_NAME='SHA256SUMS'

die() {
  printf '[%s] ERROR：%s\n' "$PROG" "$*" >&2
  exit 2
}
fail() { printf '[%s] FAIL：%s\n' "$PROG" "$*" >&2; }

MODE=""
ROOT=""
while [ "$#" -gt 0 ]; do
  case "$1" in
    --check) MODE=check ;;
    --write) MODE=write ;;
    --root)
      [ "$#" -ge 2 ] || die "--root 需要参数 DIR"
      ROOT="$2"
      shift
      ;;
    --root=*)
      ROOT="${1#--root=}"
      ;;
    -h | --help)
      sed -n '3,12p' "$0" | sed 's/^# \{0,1\}//'
      exit 0
      ;;
    *) die "未知参数：$1（支持 --check / --write / --root DIR / -h）" ;;
  esac
  shift
done
[ -n "$MODE" ] || die '必须显式指定 --check 或 --write（不带模式参数=什么都不做，是最坏的门禁形态）'

if [ -z "$ROOT" ]; then
  # 由脚本自身位置推导仓库根：镜像副本里以 `bash .meta/gates/archive-seal.sh` 调用时同样成立
  # （不绑定绝对路径——否则"检出态无法自证"）。
  ROOT=$(cd "$(dirname "$0")/../.." 2>/dev/null && pwd) || die "无法从脚本位置推导仓库根"
else
  [ -d "$ROOT" ] || die "--root 指向的目录不存在：$ROOT"
  ROOT=$(cd "$ROOT" && pwd) || die "无法进入 $ROOT"
fi

ARCHIVED="$ROOT/$ARCHIVED_REL"
MANIFEST="$ARCHIVED/$MANIFEST_NAME"
[ -d "$ARCHIVED" ] || die "缺目录 $ARCHIVED_REL（本仓无归档区，本门无对象）"
command -v sha256sum >/dev/null 2>&1 || die "缺 sha256sum（无法计算封印哈希）"

cd "$ROOT" || die "无法进入 $ROOT"

# 清单内的相对路径一律相对**仓库根**（形如 .agents/notes/archived/x.md），
# 这样同一份 manifest 在原仓与镜像副本里都成立。
list_sealed_files() {
  # 两个细节都是判据的一部分，别"顺手简化"：
  # ① 只排除**顶层**那份清单（`! -path "$MANIFEST"`）。用 `! -name SHA256SUMS` 会把任意层级
  #    的同名文件一起剔除 → `archived/sub/SHA256SUMS` 永远不会被判「未封印」（漏判）。
  # ② `-type l` 一并纳入：符号链接也是归档内容（有人可以用它把"被封印的历史"指向新内容）。
  #    注意 `sha256sum` 对链接取**目标文件内容**，故"链接指向变了但目标内容没变"不会被判出；
  #    要覆盖链接文本本身需另立判据（见文件头「边界」）。
  # 注意用**相对**清单路径比对：find 输出相对路径，与 $MANIFEST（绝对）比会永远不相等。
  find "$ARCHIVED_REL" \( -type f -o -type l \) ! -path "$ARCHIVED_REL/$MANIFEST_NAME" 2>/dev/null \
    | LC_ALL=C sort
}

hash_of() { # <相对路径>
  sha256sum -- "$1" 2>/dev/null | awk '{print $1}'
}

do_write() {
  local tmp n f h
  tmp=$(mktemp "${TMPDIR:-/tmp}/archive-seal.XXXXXX") || die "mktemp 失败"
  : >"$tmp"
  while IFS= read -r f; do
    [ -n "$f" ] || continue
    h=$(hash_of "$f")
    [ -n "$h" ] || {
      rm -f "$tmp"
      die "无法计算哈希：$f（文件不可读？）"
    }
    printf '%s  %s\n' "$h" "$f" >>"$tmp"
  done < <(list_sealed_files)
  mv -f "$tmp" "$MANIFEST" || {
    rm -f "$tmp"
    die "写入 $ARCHIVED_REL/$MANIFEST_NAME 失败"
  }
  n=$(grep -c . "$MANIFEST" 2>/dev/null || true)
  printf '[%s] 已封印：%s 个文件 → %s/%s\n' "$PROG" "${n:-0}" "$ARCHIVED_REL" "$MANIFEST_NAME"
  exit 0
}

do_check() {
  local bad=0 n=0 line h_exp path h_now sealed listed tmp

  [ -f "$MANIFEST" ] || die "缺 $ARCHIVED_REL/$MANIFEST_NAME（封印基准不存在）→ 在归档动作的同一提交里跑 bash $0 --write"

  # ① 清单内条目：存在性 + 哈希
  while IFS= read -r line; do
    case "$line" in '' | '#'*) continue ;; esac
    h_exp="${line%% *}"
    path="${line#*  }"
    case "$path" in "$ARCHIVED_REL"/*) ;; *)
      fail "manifest 含越界路径（只允许 $ARCHIVED_REL/ 下）：$path"
      bad=1
      continue
      ;;
    esac
    n=$((n + 1))
    if [ ! -e "$path" ] && [ ! -L "$path" ]; then
      fail "缺失：$path（清单有、磁盘无 → 归档条目被删除或改名）"
      bad=1
      continue
    fi
    h_now=$(hash_of "$path")
    if [ "$h_now" != "$h_exp" ]; then
      fail "已改写：$path（哈希与封印不一致 → 归档快照被静默修改）"
      bad=1
    fi
  done <"$MANIFEST"

  # ② 清单外的新文件（未封印）
  tmp=$(mktemp "${TMPDIR:-/tmp}/archive-seal.XXXXXX") || die "mktemp 失败"
  list_sealed_files >"$tmp.sealed"
  sed -n 's/^[0-9a-f]\{64\}  //p' "$MANIFEST" | LC_ALL=C sort >"$tmp.listed"
  listed=$(comm -13 "$tmp.sealed" "$tmp.listed")
  sealed=$(comm -23 "$tmp.sealed" "$tmp.listed")
  if [ -n "$sealed" ]; then
    printf '%s\n' "$sealed" | while IFS= read -r f; do
      fail "未封印：$f（目录里有新文件，但不在清单里 → 归档动作须同提交 --write）"
    done
    bad=1
  fi
  [ -z "$listed" ] || printf '%s\n' "$listed" | while IFS= read -r f; do
    fail "清单悬空：$f（清单有、目录无 → 已在 ① 报缺失，此处仅提示）"
  done
  rm -f "$tmp" "$tmp.sealed" "$tmp.listed"

  if [ "$bad" -ne 0 ]; then
    printf '[%s] 补救：确认归档内容**确实该变**后跑 bash %s --write 并在同一次提交里带上新清单；\n' "$PROG" "$0" >&2
    printf '[%s]       若改动是无意的 → git checkout -- %s/<被改文件> 还原。\n' "$PROG" "$ARCHIVED_REL" >&2
    exit 1
  fi
  printf '[%s] 通过：%s 个归档条目与封印一致（%s/%s）\n' "$PROG" "$n" "$ARCHIVED_REL" "$MANIFEST_NAME"
  exit 0
}

case "$MODE" in
  write) do_write ;;
  check) do_check ;;
esac
