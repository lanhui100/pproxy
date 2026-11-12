#!/usr/bin/env bash
# ============================================================
# A7 —— 归档封印门（A6）的负样本执行判据（门禁资产 A7）
#
# 契约（.meta/gates/README.md「负样本 spec 的两种形态」）：
#   - 自包含：以**自身所在目录**为基准寻址 A6（同目录 archive-verify.sh），
#     不依赖调用者 cwd，也不从 PATH 找同名脚本；
#   - 只读仓库：样本一律建在 `mktemp -d` 临时目录，退出时清理；不改被检仓库任何文件；
#   - 秒级：自包裹 `timeout 30`（有 timeout 时）；不调用自身（无递归）；
#   - exit 0 = 自检通过（门确实会拒绝五类非法样本，且会放行合法样本）；只允许 0/1。
#
# 五类负样本（每类都必须被 A6 拒绝，且配有正向对照，防「一律拒绝」的门混过）：
#   N1 篡改     归档件封印后被改一字节（长度不变的最坏情形）→ check 必须 1
#   N2 缺条目   归档件存在但 manifest 无该条目（未封印）→ check 必须 1
#   N3 孤儿     manifest 条目指向不存在的归档件 → check 必须 1
#   N4 重复     manifest 同一路径登记两行 → check 必须 1
#   N5 非追加   既有条目被改动后再 seal → seal 必须 1，且 manifest 零写入
#   合法样本    两件合规归档件 → seal 0 且 check 0
#
# 诚实条款：本 spec 只证明"门会响"，证明不了抽样无偏——抽样设计靠 review。
# 注：`PONYGO_IN_SPEC` 只是信息变量，**不作为自我跳过开关**（本 spec 恒完整执行）；
#     门 A6 对该变量无感（判定与输出不随其变化）。
# ============================================================
set -u

SPEC=archive-verify.spec
GATE_DIR=$(cd "$(dirname "$0")" && pwd) || {
  printf '[%s] FAIL：无法解析自身目录\n' "$SPEC" >&2
  exit 1
}
GATE="$GATE_DIR/archive-verify.sh"

# 秒级上限：自包裹 timeout 30，避免挂死拖死体检（用 PONYGO_SPEC_SELFRUN 防自我重入）
if [ "${PONYGO_SPEC_SELFRUN:-0}" != "1" ] && command -v timeout >/dev/null 2>&1; then
  export PONYGO_SPEC_SELFRUN=1
  exec timeout 30 bash "$0" "$@"
fi

[ -f "$GATE" ] || {
  printf '[%s] FAIL：缺被测门禁 %s（A6 未随框架交付或路径被改）\n' "$SPEC" "$GATE" >&2
  exit 1
}

TMP=$(mktemp -d 2>/dev/null) || {
  printf '[%s] FAIL：mktemp -d 失败\n' "$SPEC" >&2
  exit 1
}
trap 'rm -rf "$TMP" 2>/dev/null || true' EXIT INT TERM

FAIL=0
# 通过行（A7 的 [ok] 行数被上游计数：5 负样本 + 1 合法样本 = 6）
ok_case() { printf '[ok]   %s\n' "$*"; }
fail_case() {
  FAIL=1
  printf '[fail] %s\n' "$*"
  printf '[%s] 失败：%s\n' "$SPEC" "$*" >&2
}

# 造一个独立样本仓：只放被测门禁 + 归档目录（不需要 git，A6 以自身路径解析仓库根）
new_repo() { # <name> → 目录路径
  local d="$TMP/$1"
  mkdir -p "$d/.meta/gates" "$d/.agents/notes/archived/testing" || return 1
  cp "$GATE" "$d/.meta/gates/archive-verify.sh" || return 1
  printf '%s\n' "$d"
}

mk_adr() { # <dir> <文件名> [status]
  cat >"$1/.agents/notes/archived/testing/$2" <<ADREOF
# Agent Note: archive-verify spec fixture

Status: ${3:-implemented}

## Problem

spec 夹具：用于证明封印门会拒绝非法样本。

## Decision

夹具正文（内容无关，仅用于 sha256 封印）。
ADREOF
}

run_gate() { # <dir> [check|seal] → 0/1（输出丢弃，只看退出码）
  local d="$1"
  shift
  ( cd "$d" && bash "$d/.meta/gates/archive-verify.sh" "$@" ) >"$TMP/gate.log" 2>&1
}

sha_of() { sha256sum "$1" 2>/dev/null | awk '{print $1}'; }

# 篡改一字节（长度不变、确定性）：把首字节换成不同的字符
bend_byte() { # <file>
  local f="$1" first repl
  first=$(head -c 1 "$f" 2>/dev/null)
  case "$first" in
    Z) repl='Y' ;;
    *) repl='Z' ;;
  esac
  printf '%s' "$repl" | dd of="$f" bs=1 count=1 conv=notrunc status=none 2>/dev/null || return 1
  [ "$(head -c 1 "$f" 2>/dev/null)" = "$repl" ] || return 1
  return 0
}

# ---------------- N1 篡改 ----------------
d=$(new_repo n1) || { fail_case "N1 篡改：夹具创建失败"; d=""; }
if [ -n "$d" ]; then
  mk_adr "$d" "a.md"
  if ! run_gate "$d" seal; then
    fail_case "N1 篡改：夹具 seal 未成功（正向对照失败）"
  else
    bend_byte "$d/.agents/notes/archived/testing/a.md" || fail_case "N1 篡改：夹具篡改失败"
    if run_gate "$d" check; then
      fail_case "N1 篡改：归档件被改一字节后 check 仍放行（门未拒绝哈希不符）"
    else
      ok_case "N1 篡改：封印后被改一字节 → check 拒绝（exit 1）"
    fi
  fi
fi

# ---------------- N2 缺条目（未封印） ----------------
d=$(new_repo n2) || { fail_case "N2 缺条目：夹具创建失败"; d=""; }
if [ -n "$d" ]; then
  mk_adr "$d" "a.md"
  if run_gate "$d" check; then
    fail_case "N2 缺条目：归档件未封印时 check 仍放行（门未报未封印）"
  else
    ok_case "N2 缺条目：归档件未封印 → check 拒绝（exit 1）"
  fi
fi

# ---------------- N3 孤儿（manifest 有、磁盘无） ----------------
d=$(new_repo n3) || { fail_case "N3 孤儿：夹具创建失败"; d=""; }
if [ -n "$d" ]; then
  mk_adr "$d" "a.md"
  if ! run_gate "$d" seal; then
    fail_case "N3 孤儿：夹具 seal 未成功（正向对照失败）"
  else
    rm -f "$d/.agents/notes/archived/testing/a.md"
    if run_gate "$d" check; then
      fail_case "N3 孤儿：manifest 条目指向不存在文件时 check 仍放行（门未报孤儿）"
    else
      ok_case "N3 孤儿：manifest 条目指向不存在的归档件 → check 拒绝（exit 1）"
    fi
  fi
fi

# ---------------- N4 重复条目 ----------------
d=$(new_repo n4) || { fail_case "N4 重复：夹具创建失败"; d=""; }
if [ -n "$d" ]; then
  mk_adr "$d" "a.md"
  if ! run_gate "$d" seal; then
    fail_case "N4 重复：夹具 seal 未成功（正向对照失败）"
  else
    cp "$d/.agents/notes/archived/manifest.sha256" "$TMP/man4" 2>/dev/null || true
    cat "$TMP/man4" "$TMP/man4" >"$d/.agents/notes/archived/manifest.sha256" 2>/dev/null || true
    if run_gate "$d" check; then
      fail_case "N4 重复：manifest 同一路径登记两行时 check 仍放行（门未报重复）"
    else
      ok_case "N4 重复：manifest 重复条目 → check 拒绝（exit 1）"
    fi
  fi
fi

# ---------------- N5 非追加（既有条目被改动后 seal） ----------------
d=$(new_repo n5) || { fail_case "N5 非追加：夹具创建失败"; d=""; }
if [ -n "$d" ]; then
  mk_adr "$d" "a.md"
  if ! run_gate "$d" seal; then
    fail_case "N5 非追加：夹具 seal 未成功（正向对照失败）"
  else
    before=$(sha_of "$d/.agents/notes/archived/manifest.sha256")
    bend_byte "$d/.agents/notes/archived/testing/a.md" || fail_case "N5 非追加：夹具篡改失败"
    if run_gate "$d" seal; then
      fail_case "N5 非追加：既有条目被改动后 seal 仍放行（append-only 被破坏）"
    else
      after=$(sha_of "$d/.agents/notes/archived/manifest.sha256")
      if [ "$before" = "$after" ]; then
        ok_case "N5 非追加：既有条目被改动 → seal 拒绝且 manifest 零写入"
      else
        fail_case "N5 非追加：seal 拒绝但仍改写了 manifest（篡改被洗白）"
      fi
    fi
  fi
fi

# ---------------- 合法样本（正向对照：门不得一律拒绝） ----------------
d=$(new_repo legal) || { fail_case "合法样本：夹具创建失败"; d=""; }
if [ -n "$d" ]; then
  mk_adr "$d" "a.md"
  mk_adr "$d" "b.md"
  if ! run_gate "$d" seal; then
    fail_case "合法样本：合规归档件 seal 被拒（门过度拒绝）"
  elif ! run_gate "$d" check; then
    fail_case "合法样本：封印后 check 被拒（门过度拒绝）"
  else
    ok_case "合法样本：合规归档件 seal 通过且 check 通过"
  fi
fi

rm -rf "$TMP" 2>/dev/null || true
trap - EXIT INT TERM

if [ "$FAIL" -eq 0 ]; then
  printf '== %s：通过（5 类负样本 + 1 合法样本全部符合预期）==\n' "$SPEC"
  exit 0
fi
printf '== %s：未通过（见上方 [fail] 行）==\n' "$SPEC"
exit 1
