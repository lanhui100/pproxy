#!/usr/bin/env bash
# ============================================================
# install.sh —— 可选技能模板库安装器（位置子命令，零旗标）
#
# 用法：
#   bash .meta/skills-library/install.sh list
#   bash .meta/skills-library/install.sh install <name>
#   bash .meta/skills-library/install.sh check
#
# 动词契约与退出码：
#   list            每技能一行：`<name>\t<installed|absent>\t<ok|stale|modified|->`；exit 0
#                   （技能集合 = catalog.tsv 登记的库技能 ∪ 库内 `pg-*/SKILL.md` ∪
#                    `.agents/skills/*/.ponygo-installed`；模板缺失时仍列出，便于发现缺口）
#   install <name>  复制库技能文件到 `.agents/skills/<name>/`；全部复制成功后才写
#                   `.ponygo-installed`。0 成功 / 1 目标已存在（零写入）/ 2 用法错误、
#                   未知技能或无法写入（回滚本次新建目录，不留半装状态）
#   check           逐已安装项判 `ok`/`stale`/`modified`；exit 0（只读、advisory）
#
# 只读纪律：list / check 不写仓库任何文件；install 只写目标副本目录（含其 marker）。
# 永不覆盖：目标 `.agents/skills/<name>` 已存在（普通文件、目录或符号链接）即拒绝，
#   绝不改写实例文件——处理方式是 review 后手工移除副本，不由本脚本代劳。
# 三态判定顺序 modified > stale > ok；公式、marker 格式、三层归属与「不计 L3」的
#   唯一家是 `.meta/skills-library/README.md`，本脚本头注释不复述规则内容。
# 零依赖：bash 3.2+ 与 POSIX 工具（cp / mkdir / date / awk / sort）。sha256 先探测
#   `sha256sum`，再探测 `shasum -a 256`；两者皆无则拒绝运行（exit 2），不静默放行。
# ============================================================
set -u

PROG=skills-library/install
LIB=".meta/skills-library"
DEST=".agents/skills"
MARKER=".ponygo-installed"

ROOT=$(cd "$(dirname "$0")/../.." 2>/dev/null && pwd) || {
  printf '[%s] FAIL：无法解析仓库根（本脚本须位于 <repo>/.meta/skills-library/）\n' "$PROG" >&2
  exit 2
}
cd "$ROOT" || {
  printf '[%s] FAIL：无法进入仓库根 %s\n' "$PROG" "$ROOT" >&2
  exit 2
}

if command -v sha256sum >/dev/null 2>&1; then
  SHA_TOOL=sha256sum
elif command -v shasum >/dev/null 2>&1; then
  SHA_TOOL=shasum
else
  SHA_TOOL=""
  printf '[%s] FAIL：缺少 sha256 工具（需要 sha256sum 或 shasum -a 256）\n' "$PROG" >&2
  exit 2
fi

usage() {
  printf '[%s] 用法：bash .meta/skills-library/install.sh list\n' "$PROG" >&2
  printf '            bash .meta/skills-library/install.sh install <name>\n' >&2
  printf '            bash .meta/skills-library/install.sh check\n' >&2
  printf '  零旗标：不接受 --list / -l / -n 等旗标形态；三态与 marker 契约见 .meta/skills-library/README.md\n' >&2
}

die_usage() {
  printf '[%s] FAIL：%s\n' "$PROG" "$*" >&2
  usage
  exit 2
}

sha_of() {
  if [ "$SHA_TOOL" = shasum ]; then
    shasum -a 256 "$1" 2>/dev/null | awk '{print $1}'
  else
    sha256sum "$1" 2>/dev/null | awk '{print $1}'
  fi
}

# marker 单行读取：`<key>=<value>`；找不到输出空串（调用方按"不可校验"处理）。
marker_field() {
  awk -v want="$2" '
    { sub(/\r$/, "") }
    index($0, want "=") == 1 { print substr($0, length(want) + 2); exit }
  ' "$1" 2>/dev/null
}

# 库内可安装技能（`pg-*/SKILL.md` 存在即视为可安装；名字集合以磁盘为准）。
library_names() {
  local d name
  [ -d "$LIB" ] || return 0
  for d in "$LIB"/pg-*/; do
    [ -d "$d" ] || continue
    name=${d%/}
    name=${name##*/}
    [ -f "$LIB/$name/SKILL.md" ] || continue
    printf '%s\n' "$name"
  done
}

# catalog.tsv 登记的库技能名（第一列；判据真值源是 catalog，缺模板时仍可被 list 看见）。
catalog_names() {
  local f
  f="$LIB/catalog.tsv"
  [ -f "$f" ] || return 0
  LC_ALL=C awk -F'\t' '
    { sub(/\r$/, "") }
    /^#/ || /^$/ { next }
    $1 != "" { print $1 }
  ' "$f" 2>/dev/null
}

# 已安装副本（以 marker 存在为准）。
installed_names() {
  local d name
  [ -d "$DEST" ] || return 0
  for d in "$DEST"/*/; do
    [ -d "$d" ] || continue
    name=${d%/}
    name=${name##*/}
    [ -f "$DEST/$name/$MARKER" ] || continue
    printf '%s\n' "$name"
  done
}

# 三态判定（顺序即优先级）：modified > stale > ok；未安装输出 `-`。
state_of() {
  local name marker msha isha lsha
  name=$1
  marker="$DEST/$name/$MARKER"
  [ -f "$marker" ] || {
    printf '%s\n' "-"
    return 0
  }
  msha=$(marker_field "$marker" library_sha256)
  isha=$(sha_of "$DEST/$name/SKILL.md")
  if [ -z "$msha" ] || [ "$isha" != "$msha" ]; then
    printf 'modified\n'
    return 0
  fi
  lsha=$(sha_of "$LIB/$name/SKILL.md")
  if [ -z "$lsha" ] || [ "$lsha" != "$msha" ]; then
    printf 'stale\n'
    return 0
  fi
  printf 'ok\n'
}

# 失败回滚：只删本次 install 新建的副本目录（名字已过形状校验，杜绝越界 rm）。
rollback_new() {
  local name dst
  name=$1
  case "$name" in
    '' | *[!a-z0-9-]*) return 0 ;;
  esac
  dst="$DEST/$name"
  if [ -d "$dst" ]; then
    rm -rf "$dst"
  fi
  return 0
}

cmd_list() {
  local name st
  {
    catalog_names
    library_names
    installed_names
  } | LC_ALL=C sort -u | while IFS= read -r name; do
    [ -n "$name" ] || continue
    if [ -f "$DEST/$name/$MARKER" ]; then
      st=$(state_of "$name")
      printf '%s\tinstalled\t%s\n' "$name" "$st"
    else
      printf '%s\tabsent\t-\n' "$name"
    fi
  done
}

cmd_check() {
  local name st found=0
  while IFS= read -r name; do
    [ -n "$name" ] || continue
    st=$(state_of "$name")
    printf '%s\t%s\n' "$name" "$st"
    found=1
  done < <(installed_names | LC_ALL=C sort -u)
  if [ "$found" -eq 0 ]; then
    printf '[%s] 提示：无已安装副本（库技能尚未安装到 %s/）\n' "$PROG" "$DEST" >&2
  fi
}

cmd_install() {
  local name src dst f lib_sha today
  name=${1:-}
  case "$name" in
    '') die_usage "install 需要技能名（位置参数恰好 1 个）" ;;
    -*) die_usage "未知技能名：$name（零旗标；技能名形如 pg-doc-hygiene）" ;;
  esac
  case "$name" in
    *[!a-z0-9-]* | *- | *--*)
      die_usage "技能名形状非法：$name（只允许小写字母、数字、连字符，且不以连字符收尾）"
      ;;
  esac

  src="$LIB/$name/SKILL.md"
  [ -f "$src" ] || die_usage "未知技能：$name（库内无 $LIB/$name/SKILL.md；先跑 list 看可安装项；若模板缺失，用 ponygo init --repair-skeleton 补库文件）"

  dst="$DEST/$name"
  if [ -e "$dst" ] || [ -L "$dst" ]; then
    printf '[%s] 拒绝：目标已存在，不覆盖实例文件：%s\n' "$PROG" "$dst" >&2
    printf '[%s] 处理：用 check 看三态；确认副本可弃后手工移除该目录再重装\n' "$PROG" >&2
    exit 1
  fi

  lib_sha=$(sha_of "$src")
  if [ -z "$lib_sha" ]; then
    printf '[%s] FAIL：无法计算 sha256：%s（零写入）\n' "$PROG" "$src" >&2
    exit 2
  fi

  if ! mkdir -p "$DEST" 2>/dev/null; then
    printf '[%s] FAIL：无法创建目录 %s\n' "$PROG" "$DEST" >&2
    exit 2
  fi
  if ! mkdir "$dst" 2>/dev/null; then
    printf '[%s] FAIL：无法创建目标目录 %s（零写入）\n' "$PROG" "$dst" >&2
    exit 2
  fi

  for f in "$LIB/$name"/*; do
    [ -f "$f" ] || continue
    case "${f##*/}" in
      "$MARKER") continue ;;
    esac
    if ! cp "$f" "$dst/${f##*/}" 2>/dev/null; then
      printf '[%s] FAIL：复制失败：%s → %s（已回滚本次新建目录）\n' "$PROG" "$f" "$dst" >&2
      rollback_new "$name"
      exit 2
    fi
  done

  today=$(date +%Y-%m-%d 2>/dev/null) || today=""
  if [ -z "$today" ]; then
    printf '[%s] FAIL：无法取日期（date +%%Y-%%m-%%d）（已回滚本次新建目录）\n' "$PROG" >&2
    rollback_new "$name"
    exit 2
  fi

  if ! {
    printf 'library_sha256=%s\n' "$lib_sha"
    printf 'installed_at=%s\n' "$today"
  } >"$dst/$MARKER" 2>/dev/null; then
    printf '[%s] FAIL：无法写 marker %s（已回滚本次新建目录）\n' "$PROG" "$dst/$MARKER" >&2
    rollback_new "$name"
    exit 2
  fi

  printf '%s：已安装 → %s/（library_sha256=%s，installed_at=%s）\n' "$PROG" "$dst" "$lib_sha" "$today"
  printf '  三态自检：bash .meta/skills-library/install.sh check\n'
}

[ "$#" -ge 1 ] || die_usage "缺少位置子命令（list | install <name> | check）"
MODE=$1
shift

case "$MODE" in
  list)
    [ "$#" -eq 0 ] || die_usage "list 不接受额外参数（零旗标）"
    cmd_list
    ;;
  install)
    [ "$#" -eq 1 ] || die_usage "install 需要且只接受一个技能名（位置参数）"
    cmd_install "$1"
    ;;
  check)
    [ "$#" -eq 0 ] || die_usage "check 不接受额外参数（零旗标）"
    cmd_check
    ;;
  *)
    die_usage "未知输入：$MODE"
    ;;
esac
exit 0
