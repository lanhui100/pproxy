#!/usr/bin/env bash
# ============================================================
# backlog-gate.spec.sh —— backlog-gate 门禁的负样本与契约测试（v3 批次日志模型）
#
# 契约（与 .meta/gates/README.md「负样本 spec 的两种形态」一致）：
#   - 自包含：从自身路径解析仓库根，不依赖调用者 cwd；
#   - 只读仓库：样本一律建在 mktemp -d 夹具目录，退出时清理；
#   - 秒级：有 timeout 时施加 40s 上限；
#   - exit 0 = 门禁在非法样本上拒绝、在合法样本上放行（自检通过）。
#
# 断言面：
#   P1 v3 单批次合法 → 放行
#   P2 v3 双批次（并发会话各自的批次）合并 → 放行（批次日志并集不冲突）
#   P3 v2 过渡账本（tasks.jsonl，无 MODEL）→ 只读兼容放行 + NOTE
#   N1 批次首行非 batch.start → 拒绝（lint）
#   N2 未知事件类型 → 拒绝（lint）
#   N3 v2 记录缺 metadata.fingerprint → 拒绝（v2 只读 lint）
#   N4 归档批次被改动（sha256 不符）→ 拒绝（verify-archive）
#   N5 归档批次未入清单 → 拒绝（verify-archive）
#   N6 锁存而记录非 in_progress → 拒绝（租约-状态互斥）
#   N7 有 backlog/log/ 但缺 MODEL（布局不一致）→ 拒绝（fail-closed）
#   N8 无 node → fail-closed 拒绝
#   H  pre-commit 接线：门在场 → commit 放行；门缺失 → fail-closed 拒绝
# ============================================================
set -u

if [ "${PONYGO_SPEC_SELFRUN:-0}" != "1" ] && command -v timeout >/dev/null 2>&1; then
  export PONYGO_SPEC_SELFRUN=1
  exec timeout 40 bash "$0" "$@"
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
  mkdir -p "$d/scripts" "$d/backlog/log"
  cp "$CLI" "$d/scripts/backlog.mjs"
  printf 'v3\n' > "$d/backlog/MODEL"
  printf '%s\n' "$d"
}

run_gate() {
  ( cd "$1" && PONYGO_WORKSPACE_ROOT="$1" PONY_BACKLOG_ROOT="$1" bash "$GATE" )
}

# 批次夹具：$1=case 目录，$2=batch id，其余行按原样追加。
batch() {
  case_dir="$1"; batch_id="$2"; shift 2
  f="$case_dir/backlog/log/$batch_id.jsonl"
  printf '%s\n' "{\"kind\":\"batch.start\",\"model\":\"backlog-v3\",\"batch_id\":\"$batch_id\",\"surface\":\"tui\",\"created_at\":\"2026-01-01T00:00:00.000Z\"}" > "$f"
  for line in "$@"; do printf '%s\n' "$line" >> "$f"; done
}

task_line() {
  printf '%s' "{\"kind\":\"record\",\"at\":\"2026-01-01T00:00:01.000Z\",\"task\":{\"id\":\"$1\",\"category\":\"Task\",\"priority\":\"P1\",\"status\":\"todo\",\"title\":\"t\",\"created_at\":\"2026-01-01T00:00:01.000Z\",\"metadata\":{\"fingerprint\":\"f\"}}}"
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

# P1 v3 单批次合法 → 放行
c=$(mkcase pass-v3)
batch "$c" "20260101T000000Z-tui-abcd" "$(task_line B001)"
expect_pass "$c" "P1 v3 单批次合法账本"

# P2 双批次并集（并发会话各自的批次）→ 放行：批次日志没有共享写入点
c=$(mkcase pass-two-batches)
batch "$c" "20260101T000000Z-tui-abcd" "$(task_line B001)"
batch "$c" "20260101T000001Z-frontend-ef01" "$(task_line B002)"
expect_pass "$c" "P2 双批次并集"

# P3 v2 过渡账本 → 只读兼容放行 + NOTE
c="$TMPDIR_SPEC/legacy-v2"
mkdir -p "$c/scripts" "$c/backlog"
cp "$CLI" "$c/scripts/backlog.mjs"
printf '%s\n' '{"id":"B001","category":"Task","priority":"P1","status":"todo","title":"t","created_at":"2026-01-01T00:00:00.000Z","metadata":{"fingerprint":"f"}}' > "$c/backlog/tasks.jsonl"
printf '5\n' > "$c/backlog/.next-id"
if ! run_gate "$c" >"$c/gate.out" 2>&1; then
  cat "$c/gate.out" >&2
  fail "P3 v2 过渡账本未被放行"
fi
grep -qF 'v2' "$c/gate.out" || fail "P3 v2 过渡提示缺失"

# N1 批次首行非 batch.start → 拒绝
c=$(mkcase neg-no-batch-start)
printf '%s\n' "$(task_line B001)" > "$c/backlog/log/20260101T000000Z-tui-abcd.jsonl"
expect_fail "$c" "N1 批次首行非 batch.start" "首行必须是 batch.start"

# N2 未知事件类型 → 拒绝
c=$(mkcase neg-unknown-event)
batch "$c" "20260101T000000Z-tui-abcd" '{"kind":"bogus","at":"2026-01-01T00:00:02.000Z"}'
expect_fail "$c" "N2 未知事件类型" "未知事件"

# N3 v2 记录缺 fingerprint → 拒绝（v2 只读 lint 仍在守）
c="$TMPDIR_SPEC/neg-v2-fingerprint"
mkdir -p "$c/scripts" "$c/backlog"
cp "$CLI" "$c/scripts/backlog.mjs"
printf '%s\n' '{"id":"B001","category":"Task","priority":"P1","status":"todo","title":"t","created_at":"2026-01-01T00:00:00.000Z","metadata":{}}' > "$c/backlog/tasks.jsonl"
printf '5\n' > "$c/backlog/.next-id"
expect_fail "$c" "N3 v2 缺 fingerprint" "fingerprint"

# N4 归档批次被改动（sha256 不符）→ 拒绝
c=$(mkcase neg-archive-tamper)
batch "$c" "20260101T000000Z-tui-abcd" "$(task_line B001)"
mkdir -p "$c/backlog/archive/batches/2026"
mv "$c/backlog/log/20260101T000000Z-tui-abcd.jsonl" "$c/backlog/archive/batches/2026/20260101T000000Z-tui-abcd.jsonl"
printf '0000000000000000000000000000000000000000000000000000000000000000  backlog/archive/batches/2026/20260101T000000Z-tui-abcd.jsonl\n' > "$c/backlog/archive/batches/2026/manifest.sha256"
expect_fail "$c" "N4 归档批次被改动" "sha256 不符"

# N5 归档批次未入清单 → 拒绝
c=$(mkcase neg-archive-unlisted)
batch "$c" "20260101T000000Z-tui-abcd" "$(task_line B001)"
mkdir -p "$c/backlog/archive/batches/2026"
mv "$c/backlog/log/20260101T000000Z-tui-abcd.jsonl" "$c/backlog/archive/batches/2026/20260101T000000Z-tui-abcd.jsonl"
: > "$c/backlog/archive/batches/2026/manifest.sha256"
expect_fail "$c" "N5 归档批次未入清单" "未入清单"

# N6 锁存而记录非 in_progress → 租约-状态互斥拒绝
c=$(mkcase neg-lease-status-mismatch)
batch "$c" "20260101T000000Z-tui-abcd" "$(task_line B001)"
mkdir -p "$c/backlog/.locks"
printf '%s' '{"task_id":"B001","owner":"me","session_id":"me","pid":1,"acquired_at":"2026-01-01T00:00:00.000Z","heartbeat_at":"2026-01-01T00:00:00.000Z","ttl_ms":7200000}' > "$c/backlog/.locks/B001.lock"
expect_fail "$c" "N6 锁存而记录非 in_progress" "存在租约锁但记录状态"

# N7 有 backlog/log/ 但缺 MODEL → 拒绝（布局不一致）
c="$TMPDIR_SPEC/neg-layout"
mkdir -p "$c/scripts" "$c/backlog/log"
cp "$CLI" "$c/scripts/backlog.mjs"
batch "$c" "20260101T000000Z-tui-abcd" "$(task_line B001)"
expect_fail "$c" "N7 有 log 缺 MODEL" "布局不一致"

# N8 无 node → fail-closed 拒绝
c=$(mkcase no-node)
batch "$c" "20260101T000000Z-tui-abcd" "$(task_line B001)"
if env -i PATH=/nonexistent PONYGO_WORKSPACE_ROOT="$c" PONY_BACKLOG_ROOT="$c" "$(command -v bash)" "$GATE" >"$c/gate.out" 2>&1; then
  cat "$c/gate.out" >&2
  fail "N8 无 node 未被拒绝（fail-open）"
fi
grep -qF 'fail-closed' "$c/gate.out" || fail "N8 拒绝文案缺失 fail-closed"

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
  mkdir -p .githooks .meta/gates backlog/log scripts
  cp "$ROOT/.githooks/pre-commit" .githooks/pre-commit
  chmod +x .githooks/pre-commit
  cp "$CLI" scripts/backlog.mjs
  # H fixture 必须与真实仓的 pre-commit 前提一致：本仓 pre-commit 自包含（无 scripts/git-guard.mjs），
  # 门是唯一无条件依赖，故只拷门；其它守卫按「存在才跑」由 hook 自身跳过。
  cp "$GATE" .meta/gates/backlog-gate.sh
  printf 'v3\n' > backlog/MODEL
  printf '%s\n' "{\"kind\":\"batch.start\",\"model\":\"backlog-v3\",\"batch_id\":\"20260101T000000Z-tui-abcd\",\"surface\":\"tui\",\"created_at\":\"2026-01-01T00:00:00.000Z\"}" > backlog/log/20260101T000000Z-tui-abcd.jsonl
  printf '%s\n' "$(task_line B001)" >> backlog/log/20260101T000000Z-tui-abcd.jsonl
  echo hello > a.txt
  git add a.txt
)
[ -f "$h/.meta/gates/backlog-gate.sh" ] || fail "H fixture 缺 .meta/gates/backlog-gate.sh（pre-commit 无条件依赖它）"
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

echo "backlog-gate.spec: PASS（P1–P3 放行 + N1–N8 拒绝 + H pre-commit 接线）"
exit 0
