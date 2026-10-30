#!/usr/bin/env bash
# ============================================================
# secret-scan.sh —— 凭据扫描门（把「凭据不进版本库」变成非零退出命令）
#
# 用法：
#   bash .meta/gates/secret-scan.sh                # 扫本仓**已跟踪**文件（cwd 可在仓库内任意处）
#   bash .meta/gates/secret-scan.sh --root DIR     # 扫 DIR 所指 git 仓（负样本 spec / CI 复用）
#
# 退出码：
#   0 = 无命中（放行）
#   1 = 命中（逐条打印 `file:line [类别]`）
#   2 = 环境/用法错误（非 git 仓、--root 不存在、缺 xargs、grep 出错）——fail-closed，不算放行
#
# 为什么扫描面是 `git ls-files`（而不是全盘 find）：
#   门禁要守的承诺是「凭据不进版本库」，而"进了版本库"的唯一判据就是**被 index 跟踪**。
#   未跟踪的本地文件（.env / config.json / 测试产物）本来就不出仓，扫它们只制造噪声，
#   还会诱导 `git add -f` 式绕过（那是把门当敌人，不是当契约）。CI 上 checkout 出来的树
#   就是 index 内容，因此本地层与远端层是**同一判据**，不需要第二套实现（P9：一份判据一份实现）。
#
# 为什么命中时不回显明文本体：
#   门禁输出会进终端、CI 日志、issue 附件——把 token 原样打印等于换个地方再泄漏一次。
#   故只给 `file:line [类别]` 与补救命令；要定位具体字节请在本机 `sed -n '<line>p' <file>`。
#
# 类别表（判据真源即本表；新增/改类别须同步 .meta/gates/secret-scan.spec.sh 的负样本）：
#   admin-token    pony_admin_ + >=16 位 hex                       管理面 bearer token
#   gateway-key    sk-pony- + >=16 位 hex                          网关 API key
#   tenant-token   pony_ + 恰好 32 位 hex                          数据面/租户路径 token
#   proxy-secret   X-Proxy-Secret / PROXY_SECRET 附近 >=16 位 hex  上游出口共享密钥
#
# 语义边界（P6 诚实条款）：本门是**模式扫描**，不是熵检测，也不是凭据有效性验证。
#   抓得到：「已知形状的凭据字面量进了 index」。
#   抓不到：未知前缀的自造密钥、base64/拼接混淆后入库的凭据、以及"凭据仍在生产有效但已轮换
#   出库"这类流程问题（轮换纪律归发布/运维流程，不是本门能替的）。
#   命中 = 有嫌疑，必须人工核对；零命中 ≠ 本仓无可疑凭据。
#
# 只读：不写仓库任何文件（临时文件在 TMPDIR，trap 清理）。本层预算为秒级（pre-commit 用）。
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
      sed -n '3,9p' "$0" | sed 's/^# \{0,1\}//'
      exit 0
      ;;
    *) die "未知参数：$1（支持 --root DIR / -h）" ;;
  esac
done

if [ -z "$ROOT" ]; then
  ROOT=$(git rev-parse --show-toplevel 2>/dev/null) || die "当前目录不是 git 仓库（无「已跟踪」面可扫）"
else
  [ -d "$ROOT" ] || die "--root 指向的目录不存在：$ROOT"
  git -C "$ROOT" rev-parse --show-toplevel >/dev/null 2>&1 \
    || die "--root 必须指向 git 仓库（$ROOT）：扫描面 = git ls-files，非 git 目录无「已跟踪」语义"
fi
cd "$ROOT" || die "无法进入 $ROOT"

HITS=$(mktemp "${TMPDIR:-/tmp}/secret-scan.XXXXXX") || die "mktemp 失败"
trap 'rm -f "$HITS"' EXIT

# is_periodic <hexstr>：整串是否由某个短周期重复而成（全 0、deadbeef×4 之类）
# 为什么需要它：源码里的**占位/测试向量**几乎总是周期性重复串（`000…0`、`deadbeef` 重复），
# 而真随机 token 出现周期性的概率可忽略。占位符不是凭据，当成命中只会训练人绕过门禁。
is_periodic() {
  # 注意：`local s="$1" n=${#s}` 不行——`local` 是内建命令，全部实参先展开再赋值，
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

# scan_one <类别> <ERE> [no-periodic]
# 用 `git grep`（扫描面 = 已跟踪文件，退出码三档明确：0=命中 / 1=无命中 / >1=出错）。
# 不用 `git ls-files | xargs grep`：xargs 会把分块里的"无命中(1)"聚成 123，
# 让"干净"和"出错"不可区分——门禁的退出码语义必须先于便利性成立。
scan_one() {
  local label="$1" pat="$2" mode="${3:-plain}" out rc=0 line file rest ln hex
  out=$(git grep -nIE -e "$pat" -- 2>/dev/null) || rc=$?
  if [ "$rc" -gt 1 ]; then
    die "扫描出错（git grep rc=$rc，类别 $label）——无法判定即不算放行"
  fi
  [ -n "$out" ] || return 0
  while IFS= read -r line; do
    [ -n "$line" ] || continue
    file="${line%%:*}"
    rest="${line#*:}"
    ln="${rest%%:*}"
    if [ "$mode" = "no-periodic" ]; then
      # 取该行的 32 位 hex（去掉 pony_ 前缀）做周期性判定；取不到就按命中处理（fail-closed）
      hex=$(printf '%s' "$rest" | grep -oE 'pony_[0-9a-fA-F]{32}' | head -n 1)
      if [ -n "$hex" ] && is_periodic "${hex#pony_}"; then
        continue
      fi
    fi
    printf '%s:%s [%s]\n' "$file" "$ln" "$label" >>"$HITS"
  done <<<"$out"
}

scan_one admin-token 'pony_admin_[0-9a-fA-F]{16,}'
scan_one gateway-key 'sk-pony-[0-9a-fA-F]{16,}'
scan_one tenant-token '(^|[^0-9a-zA-Z_-])pony_[0-9a-fA-F]{32}([^0-9a-fA-F]|$)' no-periodic
scan_one proxy-secret '(X-Proxy-Secret|PROXY_SECRET)[^0-9a-fA-F]{0,12}[0-9a-fA-F]{16,}'

if [ -s "$HITS" ]; then
  sort -u "$HITS" >"$HITS.sorted"
  count=$(wc -l <"$HITS.sorted" | tr -d ' ')
  printf '[%s] FAIL：发现 %s 处疑似凭据（file:line [类别]）——明文本体刻意不回显，避免二次泄漏\n' "$PROG" "$count" >&2
  cat "$HITS.sorted" >&2
  printf '[%s] 补救：把凭据替换为 <REDACTED>（保留变量名/结构）并 git add；同时按轮换清单作废旧值\n' "$PROG" >&2
  printf '[%s]       误报的修法是改类别正则（判据真源在本脚本头部类别表），不是加白名单文件\n' "$PROG" >&2
  rm -f "$HITS.sorted"
  exit 1
fi

note "通过：已跟踪文件（$(git ls-files | wc -l | tr -d ' ') 个）未见 admin-token/gateway-key/tenant-token/proxy-secret 形状的凭据"
exit 0
