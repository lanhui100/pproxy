#!/usr/bin/env bash
# ============================================================
# secret-scan.spec.sh —— 凭据扫描门的负样本执行判据
#
# 契约（.meta/gates/README.md「每个门禁的落地形态」）：
#   - 自包含：从自身路径解析仓库根，不依赖调用者 cwd；
#   - 只读仓库：样本一律建在 `mktemp -d` 临时目录（含临时 git 仓），退出时清理；
#   - 秒级：自包裹 `timeout 60`（有 timeout 时）；
#   - 防递归：环境里已有 `PONYGO_IN_SPEC=1` → 打印 SKIPPED 并 exit 0（不计入 PASS）；
#   - exit 0 = 自检通过（"门确实会拒绝非法样本，且会放行合法样本"）。
#
# 本 spec 证明的是 secret-scan.sh 的**拒绝/放行语义**，不是"凭据会不会漏"——
# 后者是模式扫描的固有边界（见 secret-scan.sh 头部「语义边界」），靠 review + 轮换纪律。
#
# 假凭据纪律：本文件源码内**不存在**任何完整的假 token 字面量——一律在运行期拼装。
# 否则 spec 自己就是一条"已跟踪文件含 token 形状"，门禁会永久自指为红（假红）。
#
# 反向控制（为什么不是恒拒绝）：case 6 只有周期性占位串 → 必须放行；
#   case 7 未跟踪文件带 token → 必须放行（扫描面 = 已跟踪，见头部「为什么扫描面是 git ls-files」）。
#   只有"该拒的拒 + 该放的放"，才说明门有分辨力。
# ============================================================
set -u

SPEC=secret-scan.spec

# --- 防递归：被 audit --run-specs / CI 以 PONYGO_IN_SPEC=1 调用 → SKIPPED ---
if [ "${PONYGO_IN_SPEC:-0}" = "1" ] && [ "${PONYGO_SPEC_SELFRUN:-0}" != "1" ]; then
  printf '[%s] SKIPPED（PONYGO_IN_SPEC=1，不计入 PASS）\n' "$SPEC"
  exit 0
fi

# --- 秒级上限：自包裹 timeout 60，避免挂死拖死体检 ---
if [ "${PONYGO_SPEC_SELFRUN:-0}" != "1" ] && command -v timeout >/dev/null 2>&1; then
  export PONYGO_SPEC_SELFRUN=1
  exec timeout 60 bash "$0" "$@"
fi

ROOT=$(cd "$(dirname "$0")/../.." && pwd) || {
  printf '[%s] FAIL：无法解析仓库根\n' "$SPEC" >&2
  exit 1
}
GATE="$ROOT/.meta/gates/secret-scan.sh"
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

# 运行期拼装的假凭据（非周期性 32 位 hex；不落字面量，避免本 spec 自指为红）
H32="$(printf '%032x' "$(((RANDOM << 15) ^ RANDOM ^ $$ ^ $(date +%s)))")"

# new_repo <目录名>：造一个最小 git 仓（只 init，不 commit）
new_repo() {
  local d="$TMP/$1"
  mkdir -p "$d"
  git -C "$d" -c init.defaultBranch=main init -q || return 1
  printf '%s\n' "$d"
}

# write_tracked <仓> <相对路径> <正文>：写文件并纳入 index（=「已跟踪」）
write_tracked() {
  mkdir -p "$1/$(dirname "$2")"
  printf '%s\n' "$3" >"$1/$2"
  git -C "$1" add -A >/dev/null 2>&1
}

# run_gate <仓>：以 --root 指定被扫仓
run_gate() { # $1=仓
  ( cd "$ROOT" && bash "$GATE" --root "$1" ) 2>&1
}

expect_reject() { # $1=用例名 $2=仓 $3=拒绝输出须含的正则
  local name="$1" d="$2" pat="$3" out rc
  out=$(run_gate "$d") && rc=0 || rc=$?
  if [ "$rc" -eq 0 ]; then
    fail "$name：门应拒绝，实得 exit 0"
    return
  fi
  if ! printf '%s' "$out" | grep -qE "$pat"; then
    fail "$name：拒绝（exit $rc）但输出缺 /$pat/ → 提示不可执行"
    printf '%s\n' "$out" | tail -n 3 | sed 's/^/      /' >&2
    return
  fi
  printf 'PASS  %s（exit %d，输出含 /%s/）\n' "$name" "$rc" "$pat"
}

expect_accept() { # $1=用例名 $2=仓
  local name="$1" d="$2" out rc
  out=$(run_gate "$d") && rc=0 || rc=$?
  if [ "$rc" -ne 0 ]; then
    fail "$name：门应放行，实得 exit $rc"
    printf '%s\n' "$out" | tail -n 3 | sed 's/^/      /' >&2
    return
  fi
  printf 'PASS  %s（exit 0）\n' "$name"
}

# ---------- 1. 干净仓 → 放行 ----------
R=$(new_repo clean) || {
  printf '[%s] FAIL：git init 失败（无法造样本仓）\n' "$SPEC" >&2
  exit 1
}
write_tracked "$R" README.md '普通仓库，无凭据。'
expect_accept "干净仓放行" "$R"

# ---------- 2. 已跟踪的 admin token → 拒绝，且定位到 file:line ----------
R=$(new_repo admin-token)
write_tracked "$R" leak.sh "ADMIN=\"Authorization: Bearer pony_admin_$H32\""
expect_reject "已跟踪 admin-token 拒绝且定位 file:line" "$R" 'leak\.sh:1 \[admin-token\]'

# ---------- 3. 已跟踪的网关 key → 拒绝 ----------
R=$(new_repo gateway-key)
write_tracked "$R" cfg.env "GW_KEY=sk-pony-$H32"
expect_reject "已跟踪 gateway-key 拒绝" "$R" 'cfg\.env:1 \[gateway-key\]'

# ---------- 4. 已跟踪的租户 token（非周期性）→ 拒绝 ----------
R=$(new_repo tenant-token)
write_tracked "$R" plan.md "数据面：\`pony_$H32\`（用后即撤）"
expect_reject "已跟踪 tenant-token 拒绝" "$R" 'plan\.md:1 \[tenant-token\]'

# ---------- 5. 已跟踪的上游共享密钥 → 拒绝 ----------
R=$(new_repo proxy-secret)
write_tracked "$R" probe.sh "curl -H \"X-Proxy-Secret: $H32\" https://example.invalid"
expect_reject "已跟踪 proxy-secret 拒绝" "$R" 'probe\.sh:1 \[proxy-secret\]'

# ---------- 6. 反向控制：只有周期性占位串 → 必须放行 ----------
# 源码里的测试向量（全 0、deadbeef 重复）不是凭据；恒拒绝会让门失去分辨力、逼人 `git add -f` 绕过。
R=$(new_repo placeholders)
Z32=$(printf '%032d' 0)
write_tracked "$R" fixtures.rs "assert!(svc.verify(\"pony_$Z32\").is_err());
let t = \"pony_deadbeefdeadbeefdeadbeefdeadbeef\";"
expect_accept "周期性占位串放行（反向控制，门非恒拒绝）" "$R"

# ---------- 7. 反向控制：未跟踪文件带 token → 放行（扫描面 = 已跟踪） ----------
R=$(new_repo untracked)
write_tracked "$R" README.md '干净文件。'
printf 'TOKEN=%s\n' "pony_admin_$H32" >"$R/local-only.sh"
expect_accept "未跟踪文件放行（扫描面 = git ls-files）" "$R"

# ---------- 8. 非 git 目录 → exit 2（fail-closed，不当放行） ----------
NOTGIT="$TMP/not-a-repo"
mkdir -p "$NOTGIT"
OUT=$( cd "$ROOT" && bash "$GATE" --root "$NOTGIT" 2>&1 ) && RC=0 || RC=$?
if [ "$RC" -eq 0 ]; then
  fail "非 git 目录应报错（exit 2），实得 exit 0——环境错误被当成放行"
elif [ "$RC" -ne 2 ]; then
  fail "非 git 目录应 exit 2，实得 exit $RC（退出码语义漂移）"
else
  printf 'PASS  非 git 目录 fail-closed（exit 2）\n'
fi

# ---------- 9. 真实仓库（无参模式）→ 放行（端到端：与 CI/pre-commit 同一判据） ----------
OUT=$( cd "$ROOT" && bash "$GATE" 2>&1 ) && RC=0 || RC=$?
if [ "$RC" -ne 0 ]; then
  fail "真实仓库无参扫描应为 0，实得 exit $RC（已跟踪文件仍含凭据形状）"
  printf '%s\n' "$OUT" | tail -n 5 | sed 's/^/      /' >&2
else
  printf 'PASS  真实仓库无参扫描放行（exit 0）\n'
fi

if [ "$FAIL" -ne 0 ]; then
  printf '[%s] 未通过：门的拒绝/放行语义与预期不符（见上方 FAIL）\n' "$SPEC" >&2
  exit 1
fi
printf '[%s] 全部通过：四类凭据被拒 + 脏文件定位 file:line + 占位串/未跟踪放行 + 非 git fail-closed + 真仓放行（exit 0）\n' "$SPEC"
exit 0
