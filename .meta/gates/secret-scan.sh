#!/usr/bin/env bash
# ============================================================
# secret-scan.sh —— 凭据扫描门（把「凭据不进版本库」变成非零退出命令）
#
# 用法：
#   bash .meta/gates/secret-scan.sh                # 扫本仓：索引 + HEAD 树（cwd 可在仓库内任意处）
#   bash .meta/gates/secret-scan.sh --root DIR     # 扫 DIR（必须是仓库根；负样本 spec / 复用）
#   SKIP_HISTORY=1 bash .meta/gates/secret-scan.sh # 应急：把「历史提交残留」降为 WARN（不降级的另一条路是重写历史）
#
# 退出码：
#   0 = 无命中（放行）
#   1 = 命中（逐条打印 `file:line [类别]`）
#   2 = 环境/用法错误（非 git 仓、--root 不是仓库根、git grep 出错）——fail-closed，不算放行
#
# 扫描面 = **索引**（`git grep --cached`）+ **HEAD 树**（`git grep HEAD`）。为什么不是工作树：
#   pre-commit 的输入是**暂存区**；「先把含凭据的版本暂存、随后把工作区改净」是完全正常的一次
#   修订动作——若门读工作树，这一版非法内容会被判干净并一路提交（首版真实缺陷，2026-10-30 审查 F1）。
#   索引 = 将要提交的字节；HEAD = 已经提交的字节。两处取并集才不漏：
#   只看索引会漏掉「已入库、本地已清理但历史未重写」的残留；只看 HEAD 会漏掉未提交的暂存内容。
#   CI 上 index == HEAD（checkout 即索引同源），故本地与远端是同一判据。
#
# HEAD 命中默认也 FAIL，但给 `SKIP_HISTORY=1` 逃生口。为什么默认阻断而不是 WARN：
#   带泄漏的历史必须在推送前被看见；为什么仍要逃生口：修历史本身要一次提交，而那次提交同样会被
#   本门拒绝 → 死锁。故默认阻断 + 显式降级（降级时如实打 WARN，不静默）。CI 不设该变量。
#
# 为什么命中时不回显明文本体：
#   门禁输出会进终端、CI 日志、issue 附件——把 token 原样打印等于换个地方再泄漏一次。
#   故只给 `file:line [类别]`；要定位具体字节请在本机 `sed -n '<line>p' <file>`。
#
# 类别表（判据真源即本表；新增/改类别须同步 .meta/gates/secret-scan.spec.sh 的负样本）：
#   admin-token    pony_admin_ + >=16 位 hex                       管理面 bearer token
#   gateway-key    sk-pony- + >=16 位 hex                          网关 API key
#   tenant-token   pony_ + 恰好 32 位 hex                          数据面/租户路径 token
#   proxy-secret   X-Proxy-Secret / PROXY_SECRET 附近 >=16 位 hex  上游出口共享密钥
#   全部类别大小写不敏感（`-i`）：HTTP 头名本身大小写不敏感，写死 `X-Proxy-Secret` 会漏掉
#   `x-proxy-secret` 这类真实变体；token 前缀也不该假设全小写。
#
# 语义边界（P6 诚实条款，别把"没报"当"没有"）：
#   1. **二进制文件不扫**（`git grep -I`）：图片/安装包/压缩件里的凭据本门看不见；
#   2. **历史提交不扫**（只扫索引 + HEAD 一棵树）：更早提交里的残留需 `git filter-repo`/BFG 专项处理；
#   3. **只做模式匹配**：不是熵检测，也不验证凭据有效性。未知前缀的自造密钥、base64/拼接混淆后的
#      凭据、以及"凭据已轮换出库但仍在生产有效"这类流程问题都抓不到（轮换归发布/运维流程）。
#   命中 = 有嫌疑，必须人工核对；零命中 ≠ 本仓无可疑凭据。
#
# 只读：不写仓库任何文件（临时文件在 TMPDIR，trap 清理）。秒级（pre-commit 用）。
# ============================================================
set -u

PROG=secret-scan

note() { printf '[%s] %s\n' "$PROG" "$*"; }
die() {
  printf '[%s] ERROR：%s\n' "$PROG" "$*" >&2
  exit 2
}

ROOT=""
while [ "$#" -gt 0 ]; do
  case "$1" in
    --root)
      [ "$#" -ge 2 ] || die "--root 需要参数 DIR"
      ROOT="$2"
      shift 2
      ;;
    --root=*)
      ROOT="${1#--root=}"
      shift
      ;;
    -h | --help)
      sed -n '3,11p' "$0" | sed 's/^# \{0,1\}//'
      exit 0
      ;;
    *) die "未知参数：$1（支持 --root DIR / -h）" ;;
  esac
done

if [ -z "$ROOT" ]; then
  ROOT=$(git rev-parse --show-toplevel 2>/dev/null) || die "当前目录不是 git 仓库（无索引/HEAD 面可扫）"
else
  [ -d "$ROOT" ] || die "--root 指向的目录不存在：$ROOT"
  git -C "$ROOT" rev-parse --show-toplevel >/dev/null 2>&1 \
    || die "--root 必须指向 git 仓库（$ROOT）：扫描面 = 索引/HEAD，非 git 目录无该语义"
  # --root 必须是**仓库根**：`git grep` 在子目录里只扫该子目录，调用方误传子目录时命中集合会缩成
  # 子集甚至空集 → 门静默 PASS（假绿）。故显式校验"给定目录 = 仓库根"，不等即报错（fail-closed）。
  ROOT_ABS=$(cd "$ROOT" && pwd -P) || die "无法解析 $ROOT 的绝对路径"
  TOP_ABS=$(cd "$(git -C "$ROOT" rev-parse --show-toplevel)" && pwd -P) || die "无法解析 $ROOT 的仓库根"
  [ "$ROOT_ABS" = "$TOP_ABS" ] \
    || die "--root 必须是仓库根：给定 $ROOT_ABS，实际根 $TOP_ABS（子目录扫描面为空 = 静默放行）"
fi

cd "$ROOT" || die "无法进入 $ROOT"

HITS_INDEX=$(mktemp "${TMPDIR:-/tmp}/secret-scan.idx.XXXXXX") || die "mktemp 失败"
HITS_HEAD=$(mktemp "${TMPDIR:-/tmp}/secret-scan.head.XXXXXX") || die "mktemp 失败"
cleanup() { rm -f "$HITS_INDEX" "$HITS_HEAD" "$HITS_INDEX.sorted" "$HITS_HEAD.sorted" "$HITS_HEAD.only"; }
trap cleanup EXIT

HAS_HEAD=0
git rev-parse --verify -q HEAD >/dev/null 2>&1 && HAS_HEAD=1

# is_periodic <hexstr>：整串是否由某个短周期重复而成（全 0、deadbeef×4 之类）
# 为什么需要它：源码里的**占位/测试向量**几乎总是周期性重复串（`000…0`、`deadbeef` 重复），
# 而真随机 token 出现周期性的概率可忽略。占位符不是凭据，当成命中只会训练人绕过门禁。
is_periodic() {
  # 注意：`local s="$1" n=${#s}` 不行——`local` 是内建命令，实参先全部展开再赋值，
  # 故 ${#s} 读到的是调用者作用域（set -u 下直接 unbound）。分成两条语句。
  local s="$1"
  local n=${#s} p i ok
  for ((p = 1; p <= n / 2; p++)); do
    [ $((n % p)) -eq 0 ] || continue
    ok=1
    for ((i = p; i < n; i++)); do
      if [ "${s:i:1}" != "${s:i-p:1}" ]; then
        ok=0
        break
      fi
    done
    [ "$ok" -eq 1 ] && return 0
  done
  return 1
}

# collect <face:index|head> <类别> <ERE> <mode:plain|no-periodic> <输出文件>
# 用 `git grep`（退出码三档明确：0=命中 / 1=无命中 / >1=出错）。
# 不用 `git ls-files | xargs grep`：xargs 会把分块里的"无命中(1)"聚成 123，
# 让"干净"和"出错"不可区分——门禁的退出码语义必须先于便利性成立。
collect() {
  local face="$1" label="$2" pat="$3" mode="$4" out_file="$5"
  local out rc=0 line file rest ln hex body
  case "$face" in
    index) out=$(git grep --cached -inIE -e "$pat" -- 2>/dev/null) || rc=$? ;;
    head) out=$(git grep -inIE -e "$pat" HEAD -- 2>/dev/null) || rc=$? ;;
    *) die "内部错误：未知扫描面 $face" ;;
  esac
  [ "$rc" -le 1 ] || die "扫描出错（$face 面 git grep rc=$rc，类别 $label）——无法判定即不算放行"
  [ -n "$out" ] || return 0
  # `git grep <tree-ish>` 的输出形如 `HEAD:path:line:内容`（路径前带树名），索引面则是
  # `path:line:内容`。统一剥掉 `HEAD:` 前缀，两个面才能同格式解析，也才能互相去重（comm）。
  if [ "$face" = "head" ]; then
    out=$(printf '%s\n' "$out" | sed 's/^HEAD://')
  fi
  while IFS= read -r line; do
    [ -n "$line" ] || continue
    file="${line%%:*}"
    rest="${line#*:}"
    ln="${rest%%:*}"
    if [ "$mode" = "no-periodic" ]; then
      # 取该行的 32 位 hex 做周期性判定（大小写不敏感，与 -i 的扫描口径一致）；
      # 取不到就按命中处理（fail-closed：宁可报，不漏报）。
      hex=$(printf '%s' "$rest" | grep -oEi 'pony_[0-9a-f]{32}' | head -n 1) || true
      if [ -n "$hex" ]; then
        body="${hex#*_}" # 截掉 pony_/PONY_ 前缀（大小写无关）
        is_periodic "$body" && continue
      fi
    fi
    printf '%s:%s [%s]\n' "$file" "$ln" "$label" >>"$out_file"
  done <<<"$out"
}

scan_category() { # <类别> <ERE> [mode]
  collect index "$1" "$2" "${3:-plain}" "$HITS_INDEX"
  if [ "$HAS_HEAD" = "1" ]; then
    collect head "$1" "$2" "${3:-plain}" "$HITS_HEAD"
  fi
}

scan_category admin-token 'pony_admin_[0-9a-f]{16,}'
scan_category gateway-key 'sk-pony-[0-9a-f]{16,}'
scan_category tenant-token '(^|[^0-9a-zA-Z_-])pony_[0-9a-f]{32}([^0-9a-f]|$)' no-periodic
scan_category proxy-secret '(x-proxy-secret|proxy_secret)[^0-9a-f]{0,12}[0-9a-f]{16,}'

BLOCK=0

# ---------- 面 1：索引（将要提交的字节） ----------
if [ -s "$HITS_INDEX" ]; then
  sort -u "$HITS_INDEX" >"$HITS_INDEX.sorted"
  BLOCK=1
  printf '[%s] FAIL：索引（= 将要提交的字节）含 %s 处疑似凭据——明文本体刻意不回显，避免二次泄漏\n' \
    "$PROG" "$(wc -l <"$HITS_INDEX.sorted" | tr -d ' ')" >&2
  cat "$HITS_INDEX.sorted" >&2
  printf '[%s] 补救：把凭据替换为 <REDACTED>（保留变量名/结构）后 git add；同时按轮换清单作废旧值\n' "$PROG" >&2
fi

# ---------- 面 2：HEAD 树（已提交内容；索引已清才单列） ----------
if [ -s "$HITS_HEAD" ]; then
  sort -u "$HITS_HEAD" >"$HITS_HEAD.sorted"
  if [ -s "$HITS_INDEX" ]; then
    comm -13 "$HITS_INDEX.sorted" "$HITS_HEAD.sorted" >"$HITS_HEAD.only"
  else
    cp "$HITS_HEAD.sorted" "$HITS_HEAD.only"
  fi
  if [ -s "$HITS_HEAD.only" ]; then
    n_hist=$(wc -l <"$HITS_HEAD.only" | tr -d ' ')
    if [ "${SKIP_HISTORY:-0}" = "1" ]; then
      printf '[%s] WARN：HEAD 树含 %s 处疑似凭据（索引已清理；SKIP_HISTORY=1 → 本次不阻断，历史仍需重写）\n' \
        "$PROG" "$n_hist" >&2
      cat "$HITS_HEAD.only" >&2
    else
      BLOCK=1
      printf '[%s] FAIL：HEAD 树（已提交内容）含 %s 处疑似凭据——索引里已没有，说明它已进入历史\n' \
        "$PROG" "$n_hist" >&2
      cat "$HITS_HEAD.only" >&2
      printf '[%s] 补救：①重写历史清掉它（git filter-repo / BFG）并作废旧凭据；\n' "$PROG" >&2
      printf '[%s]       ②若本次提交只为清理、且凭据已在轮换清单内，可临时 SKIP_HISTORY=1 放行（结论降为 WARN）\n' "$PROG" >&2
    fi
  fi
fi

if [ "$BLOCK" -ne 0 ]; then
  printf '[%s] 误报的修法是改类别正则（判据真源在本脚本头部类别表），不是加白名单文件\n' "$PROG" >&2
  exit 1
fi

note "通过：索引 $(git ls-files | wc -l | tr -d ' ') 个文件 + HEAD 树，未见 admin-token/gateway-key/tenant-token/proxy-secret 形状的凭据"
exit 0
