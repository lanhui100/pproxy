#!/usr/bin/env bash
# ============================================================
# archive-verify.sh —— 归档 SHA-256 封印门（门禁资产 A6）
#
# 用法（位置子命令，零旗标）：
#   bash .meta/gates/archive-verify.sh            # 无参 = check
#   bash .meta/gates/archive-verify.sh check      # 校验封印（只读）
#   bash .meta/gates/archive-verify.sh seal       # 追加新封印（唯一写入路径）
#   旗标形态（--check / --seal / -c / -s）一律按未知输入拒绝（exit 1）。
#
# manifest：`.agents/notes/archived/manifest.sha256`，纯文本**追加式**，每行
#   `<64hex><空格><空格><仓库相对路径>`（= `sha256sum` 原生格式；十六进制必须小写）。
#
# check（只读）：manifest 登记集 ↔ 归档实体集（`archived/` 下**递归** `*.md`，排除
#   basename 为 `README.md` / `AGENTS.md` 的文件；归档区内**任何符号链接**（含**目录**符号链接）
#   与 `$ARCH` 自身是符号链接的情形一律拒绝——目录 symlink 会让 `find` 完全看不见其中的 `.md`，
#   文件 symlink 会换掉被校验的字节来源）双向一致：
#     - 归档件哈希不符（封印后被改一字节）/ 归档件未封印（manifest 缺条目）/
#       孤儿条目（manifest 有、磁盘无）/ 重复条目 / 行格式非法（含注释行）→ exit 1；
#     - 无 archived 目录、空目录、空 manifest → exit 0（门禁只对"真的归档了"负责，
#       不替实例决定要不要归档）。
# seal（唯一写入）：先按 check 的**全部**判据校验既有条目（append-only：既有行绝不重写、
#   不排序、不删除），再只为缺失条目追加新行：
#     - 既有条目被改动（哈希不符/孤儿/重复/非法）→ 拒绝且**零写入**（不得把篡改洗白）；
#     - 新条目要求该归档件的 `Status:` 首词 ∈ {implemented, archived}（**来源代理判据**：
#       `archived/` 下合法的 archived 态同样可封印；proposed / rejected 仍拒）。
#       已知局限：不做 git rename 溯源（只允许 implemented → archived 的 `git mv`），靠 review
#       与该判据兜底；
#     - 写入是**原子替换**（临时文件 = 旧字节前缀 + 新块，同目录 mv）：追加被截断留下半截行的
#       死锁红在此不可能发生——要么旧 manifest 完整、要么新 manifest 完整；
#     - 全一致态再 seal = 追加 0 条、manifest 字节不变（幂等）。
#
# 退出码：0 = 通过 / 已追加；1 = 拒绝。只允许 0/1。
# 并发（A6-24 / NFR concurrency_lock）：seal 的 read→判定→append 整段在互斥锁内（flock 优先，
#   退化为目录原子锁），获锁后重新 scan；check 只读不加锁。
# 诚实边界（A6 的威胁模型）：封印拦的是"归档件被改动而 manifest 未同步"。若有人**同时**改归档件
#   与 manifest 行，纯内容校验无法分辨（没有 git 前缀校验就没有历史真值）——那道防线属 review 与
#   CI 的可追溯性，不由本脚本冒充。
# 只读纪律（C3）：check 不写仓库任何文件、不执行任何仓库脚本（只做 sha256 比对与文本解析）；
#   seal 只追加 manifest，中间文件走 `mktemp`（系统临时目录），仓库内不留 `.tmp` / `.new` 残留。
# 零依赖（C4）：bash + GNU 用户态（sha256sum / find / awk / sed）；不引入 jq / python。
# ============================================================
set -u

PROG=archive-verify
ARCH=".agents/notes/archived"
MANIFEST="$ARCH/manifest.sha256"
MANIFEST_REL="$MANIFEST"

ROOT=$(cd "$(dirname "$0")/../.." 2>/dev/null && pwd) || {
  printf '[%s] FAIL：无法解析仓库根（本脚本须位于 <repo>/.meta/gates/）\n' "$PROG" >&2
  exit 1
}
cd "$ROOT" || {
  printf '[%s] FAIL：无法进入仓库根 %s\n' "$PROG" "$ROOT" >&2
  exit 1
}

usage() {
  printf '[%s] 用法：bash .meta/gates/archive-verify.sh [check|seal]\n' "$PROG" >&2
  printf '  check （默认）校验归档封印；只读，不写仓库任何文件\n' >&2
  printf '  seal          为未封印的归档件追加 manifest 行（append-only，不重写既有行）\n' >&2
  printf '  零旗标：不接受 --check / --seal / -c / -s 等旗标形态\n' >&2
}

if [ "$#" -gt 1 ]; then
  printf '[%s] FAIL：多余参数（只接受一个位置子命令 check|seal）\n' "$PROG" >&2
  usage
  exit 1
fi
MODE="${1:-check}"
case "$MODE" in
  check | seal) ;;
  *)
    printf '[%s] FAIL：未知输入：%s\n' "$PROG" "$MODE" >&2
    usage
    exit 1
    ;;
esac

FAILS=0
fail() {
  printf '[%s] FAIL：%s\n' "$PROG" "$*" >&2
  FAILS=$((FAILS + 1))
}

# 归档实体集 B：archived 下递归 *.md，排除 basename README.md / AGENTS.md（任意深度）
# 输出仓库相对路径（find 的打印前缀即 `.agents/notes/archived/...`），字典序稳定。
list_archived() {
  [ -d "$ARCH" ] || return 0
  find "$ARCH" -type f -name '*.md' ! -name 'README.md' ! -name 'AGENTS.md' -print 2>/dev/null |
    LC_ALL=C sort
}

# `$ARCH` 下的**所有**符号链接（-type l，不按名字过滤——**目录**符号链接同样致命）：
# 归档区的语义是"冻结的普通文件树"。任何符号链接都会让"被校验的字节/被枚举的集合"指向
# 仓库外或别处：文件 symlink 换掉内容来源；**目录 symlink** 更狠——`find` 默认不跟随，
# 于是 `find -type f -name '*.md'` 完全看不见链接目录里的 `.md`（未封印件凭空隐身），
# 而它们的可见性又随目标目录变化。故一律拒绝（C3 / F9 同源的诚实边界）。
list_archived_symlinks() {
  [ -d "$ARCH" ] || return 0
  find "$ARCH" -type l -print 2>/dev/null | LC_ALL=C sort
}

scan_symlinks() {
  local f rc=0 kind=""
  # 归档根本身是符号链接：整个判定面都在仓库外 → 直接拒绝（dir/file 都不放行）
  if [ -L "$ARCH" ]; then
    fail "归档目录是符号链接（拒绝：判定面被指向仓库外，'$ARCH' 必须是仓库内的普通目录）→ 换回普通目录后重跑"
    return 1
  fi
  while IFS= read -r f; do
    [ -n "$f" ] || continue
    if [ -d "$f" ]; then kind="目录"; else kind="文件"; fi
    fail "归档区含符号链接（$kind 链接；拒绝：链接目标可被换掉，被校验/枚举的字节不是仓库内内容）→ 换成真实$kind后重跑：$f"
    rc=1
  done < <(list_archived_symlinks)
  return "$rc"
}

sha_of() { sha256sum "$1" 2>/dev/null | awk '{print $1}'; }

# 路径必须是**仓库相对**且落在归档目录内、不得越界
path_ok() {
  case "$1" in
    /*) return 1 ;;
    .. | ../* | */.. | */../*) return 1 ;;
  esac
  case "$1" in
    "$ARCH"/*) return 0 ;;
    *) return 1 ;;
  esac
}

# 扫描 manifest（check 与 seal 共用同一套判据；唯一实现）
# 结果：SEEN（换行包裹 + 换行分隔的已登记路径，供"是否已登记"查询）
# 返回：0 = 全部条目自洽；1 = 存在格式/重复/孤儿/哈希不符
SEEN='
'
manifest_scan() {
  local line h p n=0 rc=0 real
  SEEN='
'
  [ -f "$MANIFEST" ] || return 0
  while IFS= read -r line || [ -n "$line" ]; do
    n=$((n + 1))
    # CRLF 容忍：core.autocrlf=true 的检出会让行尾带 \r；读取时归一，写入恒为 LF。
    line="${line%$'\r'}"
    # 空行（含仅空白行）忽略，不参与判定（A6-8）
    case "$line" in
      *[![:space:]]*) ;;
      *) continue ;;
    esac
    if ! [[ "$line" =~ ^([0-9a-f]{64})\ \ (.+)$ ]]; then
      fail "manifest 格式非法（第 $n 行；期望 <64hex><空格><空格><仓库相对路径>，十六进制须小写；不支持注释行）：$line"
      rc=1
      continue
    fi
    h="${BASH_REMATCH[1]}"
    p="${BASH_REMATCH[2]}"
    if ! path_ok "$p"; then
      fail "manifest 条目路径非法（第 $n 行；须为 $ARCH/ 下的仓库相对路径，不得越界）：$p"
      rc=1
      continue
    fi
    case "$SEEN" in
      *"
$p
"*)
        fail "manifest 重复条目（同一路径登记了多次）：$p"
        rc=1
        continue
        ;;
    esac
    SEEN="$SEEN$p
"
    if [ ! -f "$p" ]; then
      fail "manifest 孤儿条目（manifest 有、磁盘无）：$p"
      rc=1
      continue
    fi
    real=$(sha_of "$p")
    if [ -z "$real" ] || [ "$real" != "$h" ]; then
      fail "归档件哈希不符（封印后被改动；期望 ${h}，实得 ${real:-<不可读>}）：$p"
      rc=1
      continue
    fi
  done <"$MANIFEST"
  return "$rc"
}

is_seen() { # <path> → 0 = 已登记
  case "$SEEN" in
    *"
$1
"*) return 0 ;;
  esac
  return 1
}

# 未封印：归档实体集 B 中有、manifest 登记集 A 中无
scan_unsealed() {
  local f rc=0
  while IFS= read -r f; do
    [ -n "$f" ] || continue
    if is_seen "$f"; then continue; fi
    fail "归档件未封印（manifest 缺条目）：$f"
    rc=1
  done < <(list_archived)
  return "$rc"
}

do_check() {
  printf '== 归档封印校验（%s）==\n' "$MANIFEST_REL"
  local rc=0
  if [ -L "$ARCH" ]; then
    fail "归档目录是符号链接（拒绝：判定面被指向仓库外，'$ARCH' 必须是仓库内的普通目录）→ 换回普通目录后重跑"
    printf '[%s] 拒绝：归档目录是符号链接（见上）→ 恢复为仓库内普通目录后重跑 check\n' "$PROG" >&2
    exit 1
  fi
  if [ ! -d "$ARCH" ]; then
    if arch_deleted_vs_head; then
      fail "归档目录被删除（HEAD 有归档内容、工作区 ${ARCH} 已不存在；拒绝：归档是冻结资产，整块删除必须显式裁决）→ 恢复该目录（git checkout -- ${ARCH}），或确有裁决时用 'git commit --no-verify' 提交该删除并同步删除 manifest"
      printf '[%s] 拒绝：归档目录被删除（见上）→ 恢复目录后重跑 check\n' "$PROG" >&2
      exit 1
    fi
    printf '无归档件（目录 %s 不存在；0 条）—— 无需封印校验\n' "$ARCH"
    printf '== 归档封印：通过（无归档）==\n'
    exit 0
  fi
  manifest_scan || rc=1
  scan_symlinks || rc=1
  scan_unsealed || rc=1
  if [ "$rc" -ne 0 ] || [ "$FAILS" -ne 0 ]; then
    printf '[%s] 拒绝：归档封印不自洽（见上方 FAIL 行）→ 归档件被改动则恢复原字节；未封印件则跑 bash .meta/gates/archive-verify.sh seal\n' "$PROG" >&2
    exit 1
  fi
  local n=0
  while IFS= read -r _; do n=$((n + 1)); done < <(list_archived)
  if [ "$n" -eq 0 ]; then
    printf '无归档件（0 条）—— 无需封印校验\n'
  else
    printf '通过：%s 条归档件与 manifest 逐一一致（sha256）\n' "$n"
  fi
  printf '== 归档封印：通过 ==\n'
  exit 0
}

# 归档目录整体消失的检测（可选加固）：git 可用、HEAD 里确有归档内容、而工作区 `$ARCH` 已不存在
# → 这是"整块归档被删"，不是"从没归档过"。判据只读 git（`ls-tree`），不执行仓库脚本；
# 非 git 仓 / 新仓（HEAD 无归档记录）→ 返回 1，走"无归档 → exit 0"的老路径。
arch_deleted_vs_head() {
  git rev-parse --is-inside-work-tree >/dev/null 2>&1 || return 1
  git rev-parse --verify HEAD >/dev/null 2>&1 || return 1
  [ -n "$(git ls-tree -r --name-only HEAD -- "$ARCH" 2>/dev/null)" ] && return 0
  return 1
}

status_first_word() { # <文件> → Status: 的首词（无 Status 行则空）
  sed -n 's/^Status:[[:space:]]*//p' "$1" 2>/dev/null | head -n1 | awk '{print $1}'
}

# ---------- seal 的互斥锁（read → 判定 → append 必须串行） ----------
# 两个并发 seal 会各自读到"未登记"并各追加同一行 → manifest 重复条目，随后 check 报重复
# （门禁自伤：封印门被自己的写路径弄红）。故整个「scan → 判定 → append」段落在锁内完成，且
# **获锁后重新 scan**（锁外读到的一切都已过期）。
# 锁位置优先级：① `flock`（可用且有 .git）→ `.git/ponygo-seal.lock`（不进工作区、不进版本库）；
#               ② 否则 `$ARCH/.manifest.lock` **目录原子锁**（mkdir 的原子性）+ 超时 + trap 清理。
# check 不加锁（只读，不加锁也就不会被锁拖慢/被残留锁卡住）。
SEAL_LOCK_FD=""
SEAL_LOCK_DIR=""
SEAL_TMP_FILE=""
seal_unlock() {
  if [ -n "${SEAL_TMP_FILE:-}" ]; then
    rm -f "$SEAL_TMP_FILE" 2>/dev/null || true
    SEAL_TMP_FILE=""
  fi
  if [ -n "${SEAL_LOCK_FD:-}" ]; then
    # 只关 fd 9，**不得**追加 `2>/dev/null`：`exec` 的重定向会永久作用于当前 shell，
    # 之后所有诊断（含锁超时）都会被吞掉，seal 变成"返回 1 但零诊断"。
    exec 9>&-
    SEAL_LOCK_FD=""
  fi
  if [ -n "${SEAL_LOCK_DIR:-}" ]; then
    rmdir "$SEAL_LOCK_DIR" 2>/dev/null || rm -rf "$SEAL_LOCK_DIR" 2>/dev/null || true
    SEAL_LOCK_DIR=""
  fi
  return 0
}
seal_lock() {
  local lf="$ROOT/.git/ponygo-seal.lock" i=0
  if command -v flock >/dev/null 2>&1 && [ -d "$ROOT/.git" ]; then
    if ! { exec 9>>"$lf"; } 2>/dev/null; then
      printf '[%s] FAIL：无法创建锁文件 %s（权限/磁盘/只读挂载）→ 检查 .git 目录可写后重跑 seal\n' "$PROG" "$lf" >&2
      return 1
    fi
    if ! flock -w 10 9; then
      exec 9>&-
      printf '[%s] FAIL：等待归档封印锁超时（%s 被占用 >10s）→ 确认没有并发 seal 后重跑\n' "$PROG" "$lf" >&2
      return 1
    fi
    SEAL_LOCK_FD=9
    return 0
  fi
  # 无 flock（或无 .git）：目录原子锁（mkdir 成功即获锁；失败即被占用）
  SEAL_LOCK_DIR="$ARCH/.manifest.lock"
  if [ ! -d "$ARCH" ]; then
    # 无归档目录 = 没有任何可封印的文件；不加锁也不建目录，直接走"追加 0 条"路径
    SEAL_LOCK_DIR=""
    return 0
  fi
  while ! mkdir "$SEAL_LOCK_DIR" 2>/dev/null; do
    i=$((i + 1))
    if [ "$i" -ge 100 ]; then
      SEAL_LOCK_DIR=""
      printf '[%s] FAIL：等待归档封印锁超时（%s 被占用约 10s）→ 若无 seal 在跑，该锁目录是异常退出残留，手工删除后重跑\n' "$PROG" "$ARCH/.manifest.lock" >&2
      return 1
    fi
    sleep 0.1 2>/dev/null || sleep 1
  done
  return 0
}
seal_lock_traps() {
  trap 'seal_unlock' EXIT
  trap 'seal_unlock; trap - EXIT INT TERM; exit 130' INT
  trap 'seal_unlock; trap - EXIT INT TERM; exit 143' TERM
}

do_seal() {
  printf '== 归档封印追加（%s）==\n' "$MANIFEST_REL"
  # 0a) 归档根不得是符号链接：判定面被指向仓库外时，任何"追加"都可能写进非预期目录
  if [ -L "$ARCH" ]; then
    fail "归档目录是符号链接（拒绝：判定面被指向仓库外，'$ARCH' 必须是仓库内的普通目录）→ 换回普通目录后重跑"
    printf '[%s] 拒绝：归档目录是符号链接（见上）→ manifest 零写入\n' "$PROG" >&2
    exit 1
  fi
  # 0b) 互斥：整段 read-modify-append 串行（并发 seal 会写出重复行 → check 判红，门禁自伤）
  if ! seal_lock; then
    exit 1
  fi
  seal_lock_traps
  # 1) 既有条目必须先自洽：append-only 只允许"往后加"，不允许重写/删除/排序
  manifest_scan || {
    printf '[%s] 拒绝：既有 manifest 条目不自洽（append-only：既有行不得改动，seal 不会重写/删除它们）→ 恢复被改动的归档件，或由人工裁决后重跑\n' "$PROG" >&2
    exit 1
  }
  if [ "$FAILS" -ne 0 ]; then
    printf '[%s] 拒绝：既有 manifest 条目不自洽（append-only）→ 恢复被改动的归档件后重跑\n' "$PROG" >&2
    exit 1
  fi
  # 1b) 符号链接不得混进封印：它绕开 `find -type f`，会让"被校验的字节"指向仓库外
  if ! scan_symlinks; then
    printf '[%s] 拒绝：归档目录含符号链接（见上方 FAIL 行）→ 换成普通文件后重跑 seal；manifest 零写入\n' "$PROG" >&2
    exit 1
  fi
  # 2) 只为缺失条目构造追加块；任一新条目不合法 → 整批拒绝且零写入
  #    来源代理判据（放宽后）：Status 首词 ∈ {implemented, archived}——`archived/` 下合法的
  #    archived 态（verify-note 1.7 允许）同样可封印；proposed / rejected 仍拒（未落地的决策）。
  local f st n=0 block="" h
  block=""
  while IFS= read -r f; do
    [ -n "$f" ] || continue
    if is_seen "$f"; then continue; fi
    st=$(status_first_word "$f")
    case "$st" in
      implemented | archived) ;;
      *)
        fail "归档件 Status 非 implemented/archived（来源代理判据：只封印已落地的决策；当前 Status 首词：${st:-缺失}）：$f"
        continue
        ;;
    esac
    h=$(sha_of "$f")
    if [ -z "$h" ]; then
      fail "无法计算 sha256（文件不可读）：$f"
      continue
    fi
    block="$block$h  $f
"
    n=$((n + 1))
  done < <(list_archived)
  if [ "$FAILS" -ne 0 ]; then
    printf '[%s] 拒绝：新条目不满足封印条件（见上方 FAIL 行）→ manifest 零写入\n' "$PROG" >&2
    exit 1
  fi
  if [ "$n" -eq 0 ]; then
    printf '追加 0 条（全一致态；manifest 字节不变）\n'
    printf '== 归档封印：通过（0 条新增）==\n'
    seal_unlock
    trap - EXIT INT TERM
    exit 0
  fi
  # 3) 原子提交：临时文件 = 既有 manifest 原字节（逐字节前缀）+ 换行分隔（若末行缺）+ 新块，
  #    然后同目录 `mv` 原子替换。为什么不用 `cat >>`：追加被 ulimit/中断截断会留下**半截行**
  #    （torn line），此后 regex 判定恒失败、seal 也不可自愈（死锁红）。原子替换保证
  #    「要么旧内容完整、要么新内容完整」；既有行内容逐字节不变（append-only 在内容层成立）。
  mkdir -p "$ARCH" || {
    printf '[%s] FAIL：无法创建 %s\n' "$PROG" "$ARCH" >&2
    exit 1
  }
  local newf=""
  # 后缀必须是 `.tmp`：冻结断言的残留判据是 `-name '*.tmp'`，且 SIGKILL 残留也一眼可辨
  # （`.manifest.XXXXXX.tmp` 同样非 `.md`，不会被归档实体集/摘要统计计入）。
  newf=$(mktemp "$ARCH/.manifest.XXXXXX.tmp" 2>/dev/null) || newf=""
  if [ -z "$newf" ]; then
    printf '[%s] FAIL：无法创建临时文件（%s/.manifest.XXXXXX.tmp）——本次未写入 manifest\n' "$PROG" "$ARCH" >&2
    exit 1
  fi
  SEAL_TMP_FILE="$newf"
  if ! {
    if [ -s "$MANIFEST" ]; then
      cat "$MANIFEST" || exit 1
      if [ -n "$(tail -c 1 "$MANIFEST" 2>/dev/null)" ]; then printf '\n' || exit 1; fi
    fi
    printf '%s' "$block" || exit 1
  } >"$newf"; then
    printf '[%s] FAIL：写临时 manifest 失败（磁盘/权限）——本次未写入，旧 manifest 保持完整\n' "$PROG" >&2
    exit 1
  fi
  chmod 644 "$newf" 2>/dev/null || true
  if ! mv -f "$newf" "$MANIFEST" 2>/dev/null; then
    printf '[%s] FAIL：原子替换 manifest 失败（同目录 mv 失败）——旧 manifest 保持完整\n' "$PROG" >&2
    exit 1
  fi
  SEAL_TMP_FILE=""
  printf '追加 %s 条新封印（既有条目零改动）\n' "$n"
  printf '== 归档封印：通过（%s 条新增）==\n' "$n"
  seal_unlock
  trap - EXIT INT TERM
  exit 0
}

case "$MODE" in
  check) do_check ;;
  seal) do_seal ;;
esac
