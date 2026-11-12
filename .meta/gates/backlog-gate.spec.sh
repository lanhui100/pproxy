#!/usr/bin/env bash
# ============================================================
# backlog-gate.spec.sh —— backlog-gate 门禁的负样本与契约测试
#
# 契约（与 .meta/gates/README.md「负样本 spec 的两种形态」一致）：
#   - 自包含：从自身路径解析仓库根，不依赖调用者 cwd；
#   - 只读仓库：样本一律建在 mktemp -d 夹具目录，退出时清理；
#   - 秒级：有 timeout 时施加 30s 上限；
#   - exit 0 = 门禁在非法样本上拒绝、在合法样本上放行（自检通过）。
#
# 断言面：
#   P1 合法账本 → 放行
#   N1 记录缺 metadata.fingerprint → 拒绝（lint）
#   N2 活跃池残留 done → 拒绝（lint）
#   N3 孤儿租约 → 拒绝（doctor 对账）
#   N4 记录 ID 越过高水位 → 拒绝（lint）
#   N5 无账本 → 跳过并放行（迁移期语义）
#   N6 无 node → fail-closed 拒绝
#   N7 锁存在但记录非 in_progress（手改降级 / start 中断）→ 拒绝（租约-状态互斥）
#   H  pre-commit 接线：门在场 → commit 放行；门缺失 → fail-closed 拒绝
# ============================================================
set -u

if [ "${PONYGO_SPEC_SELFRUN:-0}" != "1" ] && command -v timeout >/dev/null 2>&1; then
  export PONYGO_SPEC_SELFRUN=1
  exec timeout 30 bash "$0" "$@"
fi

ROOT=$(cd "$(dirname "$0")/../.." && pwd)
GATE="$ROOT/.meta/gates/backlog-gate.sh"
CLI="$ROOT/scripts/backlog.mjs"

TMPDIR_SPEC=$(mktemp -d)
trap 'rm -rf "$TMPDIR_SPEC"' EXIT

fail() { echo "spec: FAIL $*" >&2; exit 1; }

# 每个样本是独立夹具仓：只放被测 CLI 副本与账本，门禁按 PONYGO_WORKSPACE_ROOT 寻址。
mkcase() {
  d="$TMPDIR_SPEC/$1"
  mkdir -p "$d/scripts" "$d/backlog"
  cp "$CLI" "$d/scripts/backlog.mjs"
  printf '%s\n' "$d"
}

run_gate() {
  ( cd "$1" && PONYGO_WORKSPACE_ROOT="$1" PONY_BACKLOG_ROOT="$1" bash "$GATE" )
}

valid_record() {
  printf '%s\n' '{"id":"B001","category":"Task","priority":"P1","status":"todo","title":"t","created_at":"2026-01-01T00:00:00.000Z","metadata":{"fingerprint":"f"}}'
}

expect_pass() {
  if ! run_gate "$1" >"$1/gate.out" 2>&1; then
    cat "$1/gate.out" >&2
    fail "$2 未被放行"
  fi
}

expect_fail() {
  case_dir="$1"; label="$2"; needle="$3"
  if run_gate "$case_dir" >"$case_dir/gate.out" 2>&1; then
    cat "$case_dir/gate.out" >&2
    fail "$label 未被拒绝"
  fi
  grep -qF "$needle" "$case_dir/gate.out" || {
    cat "$case_dir/gate.out" >&2
    fail "$label 拒绝证据不完整（未出现：$needle）"
  }
}

# P1 合法账本：todo 记录 + 高水位越过最大编号 → 放行
c=$(mkcase pass-legal)
valid_record > "$c/backlog/tasks.jsonl"
printf '5\n' > "$c/backlog/.next-id"
expect_pass "$c" "P1 合法账本"

# N1 缺 metadata.fingerprint → lint 拒绝
c=$(mkcase neg-fingerprint)
printf '%s\n' '{"id":"B001","category":"Task","priority":"P1","status":"todo","title":"t","created_at":"2026-01-01T00:00:00.000Z","metadata":{}}' > "$c/backlog/tasks.jsonl"
printf '5\n' > "$c/backlog/.next-id"
expect_fail "$c" "N1 缺 fingerprint" "fingerprint"

# N2 活跃池残留 done → 拒绝
c=$(mkcase neg-active-done)
printf '%s\n' '{"id":"B001","category":"Task","priority":"P1","status":"done","title":"t","created_at":"2026-01-01T00:00:00.000Z","evidence":"x","closed_at":"2026-01-02T00:00:00.000Z","metadata":{"fingerprint":"f"}}' > "$c/backlog/tasks.jsonl"
printf '5\n' > "$c/backlog/.next-id"
expect_fail "$c" "N2 活跃池 done 残留" "活跃池不得残留 done"

# N3 孤儿租约（活跃池无此记录）→ doctor 对账拒绝
c=$(mkcase neg-orphan-lease)
: > "$c/backlog/tasks.jsonl"
printf '5\n' > "$c/backlog/.next-id"
mkdir -p "$c/backlog/.locks"
printf '%s' '{"task_id":"B009","owner":"me","session_id":"me","pid":1,"acquired_at":"2026-01-01T00:00:00.000Z","heartbeat_at":"2026-01-01T00:00:00.000Z","ttl_ms":7200000}' > "$c/backlog/.locks/B009.lock"
expect_fail "$c" "N3 孤儿租约" "残留租约"

# N4 记录 ID 越过高水位 → lint 拒绝
c=$(mkcase neg-highwater)
printf '%s\n' '{"id":"B010","category":"Task","priority":"P1","status":"todo","title":"t","created_at":"2026-01-01T00:00:00.000Z","metadata":{"fingerprint":"f"}}' > "$c/backlog/tasks.jsonl"
printf '10\n' > "$c/backlog/.next-id"
expect_fail "$c" "N4 高水位越界" "越过高水位"

# N5 无账本 → 明确跳过并放行（迁移期语义）
c="$TMPDIR_SPEC/no-ledger"
mkdir -p "$c"
expect_pass "$c" "N5 无账本（迁移期跳过）"
grep -qF '跳过' "$c/gate.out" || fail "N5 跳过提示缺失"

# N6 无 node → fail-closed 拒绝
c=$(mkcase no-node)
valid_record > "$c/backlog/tasks.jsonl"
printf '5\n' > "$c/backlog/.next-id"
if env -i PATH=/nonexistent PONYGO_WORKSPACE_ROOT="$c" PONY_BACKLOG_ROOT="$c" "$(command -v bash)" "$GATE" >"$c/gate.out" 2>&1; then
  cat "$c/gate.out" >&2
  fail "N6 无 node 未被拒绝（fail-open）"
fi
grep -qF 'fail-closed' "$c/gate.out" || fail "N6 拒绝文案缺失 fail-closed"

# N7 锁存在但记录非 in_progress → 租约-状态互斥拒绝
c=$(mkcase neg-lease-status-mismatch)
valid_record > "$c/backlog/tasks.jsonl"
printf '5\n' > "$c/backlog/.next-id"
mkdir -p "$c/backlog/.locks"
printf '%s' '{"task_id":"B001","owner":"me","session_id":"me","pid":1,"acquired_at":"2026-01-01T00:00:00.000Z","heartbeat_at":"2026-01-01T00:00:00.000Z","ttl_ms":7200000}' > "$c/backlog/.locks/B001.lock"
expect_fail "$c" "N7 锁存而记录非 in_progress" "存在租约锁但记录状态"

# H pre-commit 接线：门在场必须放行，门缺失必须 fail-closed 拒绝
command -v git >/dev/null 2>&1 || fail "H 需要 git 才能验证 pre-commit 接线"
h="$TMPDIR_SPEC/hook-wiring"
mkdir -p "$h"
(
  cd "$h"
  git init -q
  git config user.email spec@example.com
  git config user.name spec
  git config core.hooksPath .githooks
  mkdir -p .githooks .meta/gates backlog scripts
  cp "$ROOT/.githooks/pre-commit" .githooks/pre-commit
  chmod +x .githooks/pre-commit
  cp "$CLI" scripts/backlog.mjs
  cp "$GATE" .meta/gates/backlog-gate.sh
  printf '5\n' > backlog/.next-id
  valid_record > backlog/tasks.jsonl
  echo hello > a.txt
  git add a.txt
)
if ! ( cd "$h" && git commit -qm control ) >"$h/hook.out" 2>&1; then
  cat "$h/hook.out" >&2
  fail "H 门在场时合法提交被误拒"
fi
( cd "$h" && rm -f .meta/gates/backlog-gate.sh && echo world > b.txt && git add b.txt )
if ( cd "$h" && git commit -qm blocked ) >"$h/hook2.out" 2>&1; then
  cat "$h/hook2.out" >&2
  fail "H 门缺失时提交未被 fail-closed 拒绝"
fi
grep -qF 'fail-closed' "$h/hook2.out" || fail "H 拒绝文案缺失 fail-closed"

echo "backlog-gate.spec: PASS（P1 放行 + N1–N7 全部按预期拒绝/跳过 + H pre-commit 接线）"
exit 0
