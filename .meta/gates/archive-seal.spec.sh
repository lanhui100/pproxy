#!/usr/bin/env bash
# ============================================================
# archive-seal.spec.sh —— 归档封印门的负样本执行判据
#
# 契约（.meta/gates/README.md「每个门禁的落地形态」）：
#   - 自包含：从自身路径解析被测脚本，不依赖调用者 cwd；
#   - 只读仓库：样本一律建在 `mktemp -d` 临时目录，退出时清理；
#   - 秒级：自包裹 `timeout 60`；
#   - 防递归：`PONYGO_IN_SPEC=1` → SKIPPED 且 exit 0（不计入 PASS）；
#   - exit 0 = 自检通过（该拒的拒、该放的放）。
#
# 本 spec 证明封印门的**拒绝/放行语义**，不证明"封印时内容是对的"（那是 review 的时点判断）。
# 反向控制：干净样本必须放行——只有"坏样本被拒 + 好样本放行"才说明门有分辨力，恒拒绝是假安全。
# ============================================================
set -u

SPEC=archive-seal.spec
ARCHIVED_REL='.agents/notes/archived'
MANIFEST_NAME='SHA256SUMS'

if [ "${PONYGO_IN_SPEC:-0}" = "1" ] && [ "${PONYGO_SPEC_SELFRUN:-0}" != "1" ]; then
  printf '[%s] SKIPPED（PONYGO_IN_SPEC=1，不计入 PASS）\n' "$SPEC"
  exit 0
fi
if [ "${PONYGO_SPEC_SELFRUN:-0}" != "1" ] && command -v timeout >/dev/null 2>&1; then
  export PONYGO_SPEC_SELFRUN=1
  exec timeout 60 bash "$0" "$@"
fi

ROOT=$(cd "$(dirname "$0")/../.." && pwd) || {
  printf '[%s] FAIL：无法解析仓库根\n' "$SPEC" >&2
  exit 1
}
GATE="$ROOT/.meta/gates/archive-seal.sh"
[ -f "$GATE" ] || {
  printf '[%s] FAIL：缺被测资产 %s\n' "$SPEC" "$GATE" >&2
  exit 1
}

TMP=$(mktemp -d) || {
  printf '[%s] FAIL：mktemp -d 失败\n' "$SPEC" >&2
  exit 1
}
trap 'rm -rf "$TMP"' EXIT

FAIL=0
fail() {
  printf 'FAIL  %s\n' "$*" >&2
  FAIL=1
}

# new_archive <目录名>：造一个含 archived/ 的最小根（封印门只需要这条目录 + manifest）
new_archive() {
  local d="$TMP/$1"
  mkdir -p "$d/$ARCHIVED_REL" || return 1
  printf '# 归档索引\n' >"$d/$ARCHIVED_REL/AGENTS.md"
  printf '# Agent Note: 历史决策\n\nStatus: implemented\n\n## Decision\n\n旧决定。\n' \
    >"$d/$ARCHIVED_REL/2026-01-01-old-decision.md"
  printf '%s\n' "$d"
}

run_gate() { # <root> [args...]
  local d="$1"
  shift
  bash "$GATE" --root "$d" "$@" 2>&1
}

expect_accept() { # <用例名> <root> [args...]（缺省 --check）
  local name="$1" d="$2" out rc
  shift 2
  [ "$#" -gt 0 ] || set -- --check
  out=$(run_gate "$d" "$@") && rc=0 || rc=$?
  if [ "$rc" -ne 0 ]; then
    fail "$name：应放行，实得 exit $rc"
    printf '%s\n' "$out" | tail -n 3 | sed 's/^/      /' >&2
    return
  fi
  printf 'PASS  %s（exit 0）\n' "$name"
}

expect_reject() { # <用例名> <root> <期望退出码> <输出须含正则> [args...]（缺省 --check）
  local name="$1" d="$2" want="$3" pat="$4" out rc
  shift 4
  [ "$#" -gt 0 ] || set -- --check
  out=$(run_gate "$d" "$@") && rc=0 || rc=$?
  if [ "$rc" -eq 0 ]; then
    fail "$name：应拒绝，实得 exit 0"
    return
  fi
  if [ "$rc" -ne "$want" ]; then
    fail "$name：应 exit $want，实得 exit $rc（退出码语义漂移）"
    printf '%s\n' "$out" | tail -n 3 | sed 's/^/      /' >&2
    return
  fi
  if ! printf '%s' "$out" | grep -qE "$pat"; then
    fail "$name：exit $rc 但输出缺 /$pat/（提示不可执行）"
    printf '%s\n' "$out" | tail -n 3 | sed 's/^/      /' >&2
    return
  fi
  printf 'PASS  %s（exit %d，输出含 /%s/）\n' "$name" "$rc" "$pat"
}

# ---------- 1. 反向控制：--write 后立即 --check 必须放行（回环一致） ----------
R=$(new_archive loop) || {
  printf '[%s] FAIL：无法造样本根\n' "$SPEC" >&2
  exit 1
}
expect_accept "封印后立即校验放行（回环一致）" "$R" --write
expect_accept "同一内容再校验放行" "$R" --check

# ---------- 2. 篡改归档内容 → 拒绝 ----------
R=$(new_archive tamper)
run_gate "$R" --write >/dev/null
printf '\n静默追加一行\n' >>"$R/$ARCHIVED_REL/2026-01-01-old-decision.md"
expect_reject "篡改归档条目拒绝" "$R" 1 '已改写'

# ---------- 3. 删除归档条目 → 拒绝 ----------
R=$(new_archive deleted)
run_gate "$R" --write >/dev/null
rm -f "$R/$ARCHIVED_REL/2026-01-01-old-decision.md"
expect_reject "删除归档条目拒绝" "$R" 1 '缺失'

# ---------- 4. 未封印的新文件 → 拒绝（防"绕过封印偷偷塞条目"） ----------
R=$(new_archive unsealed)
run_gate "$R" --write >/dev/null
printf '# 新归档\n' >"$R/$ARCHIVED_REL/2026-02-02-sneaky.md"
expect_reject "清单外新文件拒绝（未封印）" "$R" 1 '未封印'

# ---------- 5. 缺 manifest → exit 2（fail-closed，不当放行） ----------
R=$(new_archive no-manifest)
expect_reject "缺 manifest 时 fail-closed" "$R" 2 '缺 .agents/notes/archived/SHA256SUMS'

# ---------- 6. 不带 --check/--write → exit 2（空壳门禁形态必须被拒） ----------
R=$(new_archive no-mode)
OUT=$(run_gate "$R") && RC=0 || RC=$?
if [ "$RC" -ne 2 ]; then
  fail "未指定模式应 exit 2，实得 exit $RC"
elif ! printf '%s' "$OUT" | grep -q '必须显式指定'; then
  fail "未指定模式的拒绝文案缺「必须显式指定」（提示不可执行）"
else
  printf 'PASS  未指定模式拒绝（exit 2，文案含「必须显式指定」）\n'
fi

# ---------- 7. manifest 含越界路径 → 拒绝（封印面不许指向 archived/ 之外） ----------
R=$(new_archive escape)
Z=$(printf 'a%063d' 0)
printf '%s  crates/core/src/token.rs\n' "$Z" >"$R/$ARCHIVED_REL/$MANIFEST_NAME"
expect_reject "manifest 越界路径拒绝" "$R" 1 '越界路径'

# ---------- 8. 真实仓库：--check 必须放行（端到端，与 CI/pre-push 同判据） ----------
OUT=$( cd "$ROOT" && bash "$GATE" --check 2>&1 ) && RC=0 || RC=$?
if [ "$RC" -ne 0 ]; then
  fail "真实仓库 --check 应为 0，实得 exit $RC"
  printf '%s\n' "$OUT" | tail -n 3 | sed 's/^/      /' >&2
else
  printf 'PASS  真实仓库 --check 放行（exit 0）\n'
fi

if [ "$FAIL" -ne 0 ]; then
  printf '[%s] 未通过：封印门的拒绝/放行语义与预期不符（见上方 FAIL）\n' "$SPEC" >&2
  exit 1
fi
printf '[%s] 全部通过：篡改/删除/未封印/缺清单/缺模式/越界路径均被拒 + 干净样本与真仓放行（exit 0）\n' "$SPEC"
exit 0
