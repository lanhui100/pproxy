#!/usr/bin/env bash
# ============================================================
# secret-scan.spec.sh —— 凭据扫描门的负样本执行判据
#
# 契约（.meta/gates/README.md「每个门禁的落地形态」）：
#   - 自包含：从自身路径解析仓库根，不依赖调用者 cwd；
#   - 只读仓库：样本一律建在 `mktemp -d` 临时目录（含临时 git 仓），退出时清理；
#   - 秒级：自包裹 `timeout 60`（有 timeout 时）；
#   - 防重复执行：自包裹时以 PONYGO_SPEC_SELFRUN=1 重入一次（幂等，无递归）；
#     即使被 `ponygo audit --run-specs`（会设 PONYGO_IN_SPEC=1）调用也照常执行，
#     不做 SKIPPED——否则 2.3 的「执行判据」对本门退化为代理判据（2026-10-30 复审发现）。
#   - exit 0 = 自检通过（"门确实会拒绝非法样本，且会放行合法样本"）。
#
# 本 spec 证明的是 secret-scan.sh 的**拒绝/放行语义**，不是"凭据不会漏"——
# 后者是模式扫描的固有边界（见 secret-scan.sh 头部「语义边界」：二进制不扫、历史不扫、不做熵检测）。
#
# 扫描面相关用例（2026-10-30 审查 F1 的回归锁，最容易被"顺手优化"掉的三条）：
#   case 8  暂存含凭据 + 工作区已改净 → 必须拒绝（门读索引，不读工作树）
#   case 9  工作区脏 + 索引干净       → 必须放行（索引才是"将要提交的字节"）
#   case 10 HEAD 已入库 + 索引已清理  → 默认拒绝；SKIP_HISTORY=1 降为 WARN 放行（防提交死锁）
#   case 11 --root 传子目录           → 必须 exit 2（否则子目录扫 0 文件 = 静默假绿）
#
# 假凭据纪律：本文件源码内**不存在**任何完整的假 token 字面量——一律在运行期拼装。
# 否则 spec 自己就是一条"已跟踪文件含 token 形状"，门禁会永久自指为红（假红）。
#
# 反向控制（为什么不是恒拒绝）：case 6 只有周期性占位串 → 必须放行；
#   case 7 未跟踪文件带 token → 必须放行（扫描面 = 索引/HEAD，未跟踪内容不出仓）。
#   只有"该拒的拒 + 该放的放"，才说明门有分辨力。
# ============================================================
set -u

SPEC=secret-scan.spec

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
GIT_ID=(-c user.email=spec@example.invalid -c user.name=spec)

# new_repo <目录名>：造一个最小 git 仓（只 init）
new_repo() {
  local d="$TMP/$1"
  mkdir -p "$d"
  git -C "$d" -c init.defaultBranch=main init -q || return 1
  printf '%s\n' "$d"
}

# write_tracked <仓> <相对路径> <正文>：写文件并纳入索引（=「将随本次提交入库」）
write_tracked() {
  mkdir -p "$1/$(dirname "$2")"
  printf '%s\n' "$3" >"$1/$2"
  git -C "$1" add -A >/dev/null 2>&1
}

run_gate() { # $1=仓（--root）
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

# ---------- 2. 索引里的 admin token → 拒绝，且定位到 file:line ----------
R=$(new_repo admin-token)
write_tracked "$R" leak.sh "ADMIN=\"Authorization: Bearer pony_admin_$H32\""
expect_reject "索引含 admin-token 拒绝且定位 file:line" "$R" 'leak\.sh:1 \[admin-token\]'

# ---------- 3. 索引里的网关 key → 拒绝 ----------
R=$(new_repo gateway-key)
write_tracked "$R" cfg.env "GW_KEY=sk-pony-$H32"
expect_reject "索引含 gateway-key 拒绝" "$R" 'cfg\.env:1 \[gateway-key\]'

# ---------- 4. 索引里的租户 token（非周期性）→ 拒绝 ----------
R=$(new_repo tenant-token)
write_tracked "$R" plan.md "数据面：\`pony_$H32\`（用后即撤）"
expect_reject "索引含 tenant-token 拒绝" "$R" 'plan\.md:1 \[tenant-token\]'

# ---------- 5. 索引里的上游共享密钥（小写头名）→ 拒绝（F1：大小写不敏感） ----------
R=$(new_repo proxy-secret)
write_tracked "$R" probe.sh "curl -H \"x-proxy-secret: $H32\" https://example.invalid"
expect_reject "索引含 proxy-secret（小写头名）拒绝" "$R" 'probe\.sh:1 \[proxy-secret\]'

# ---------- 6. 反向控制：只有周期性占位串 → 必须放行 ----------
# 源码里的测试向量（全 0、deadbeef 重复）不是凭据；恒拒绝会让门失去分辨力、逼人 `git add -f` 绕过。
R=$(new_repo placeholders)
Z32=$(printf '%032d' 0)
write_tracked "$R" fixtures.rs "assert!(svc.verify(\"pony_$Z32\").is_err());
let t = \"pony_deadbeefdeadbeefdeadbeefdeadbeef\";"
expect_accept "周期性占位串放行（反向控制，门非恒拒绝）" "$R"

# ---------- 7. 反向控制：未跟踪文件带 token → 放行（扫描面 = 索引/HEAD） ----------
R=$(new_repo untracked)
write_tracked "$R" README.md '干净文件。'
printf 'TOKEN=%s\n' "pony_admin_$H32" >"$R/local-only.sh"
expect_accept "未跟踪文件放行（扫描面 = 索引/HEAD）" "$R"

# ---------- 8. F1 核心：暂存含凭据 + 工作区已改净 → 必须拒绝 ----------
# "git add 了含 token 的版本，随后在编辑器里脱敏但没再 add" —— 这一步最容易漏报：
# 门若读工作树会判干净，而真正会进仓的是索引里那一版。
R=$(new_repo staged-then-cleaned)
write_tracked "$R" leak.sh "ADMIN=\"Authorization: Bearer pony_admin_$H32\""
printf 'ADMIN="Authorization: Bearer <REDACTED>"\n' >"$R/leak.sh" # 工作区已脱敏，但**没有** git add
expect_reject "暂存含凭据 + 工作区已改净 → 拒绝（门读索引）" "$R" 'leak\.sh:1 \[admin-token\]'

# ---------- 9. F1 反向：工作区脏（未暂存）+ 索引干净 → 必须放行 ----------
R=$(new_repo worktree-dirty)
write_tracked "$R" cfg.env 'GW_KEY=<REDACTED>'
printf 'GW_KEY=%s\n' "sk-pony-$H32" >>"$R/cfg.env" # 只在工作区，未 git add
expect_accept "工作区脏 + 索引干净 → 放行（未暂存内容不进本次提交）" "$R"

# ---------- 10. F1 并集：HEAD 已入库 + 索引已清理 → 默认拒绝；SKIP_HISTORY=1 降为 WARN ----------
R=$(new_repo history-residue)
write_tracked "$R" leak.sh "ADMIN=\"Authorization: Bearer pony_admin_$H32\""
git -C "$R" "${GIT_ID[@]}" commit -qm "leak" >/dev/null 2>&1
printf 'ADMIN="Authorization: Bearer <REDACTED>"\n' >"$R/leak.sh"
git -C "$R" add -A >/dev/null 2>&1
expect_reject "HEAD 已入库 + 索引已清理 → 默认拒绝（历史残留）" "$R" 'leak\.sh:1 \[admin-token\]'
OUT=$( cd "$ROOT" && SKIP_HISTORY=1 bash "$GATE" --root "$R" 2>&1 ) && RC=0 || RC=$?
if [ "$RC" -ne 0 ]; then
  fail "SKIP_HISTORY=1 应把历史残留降为 WARN 并放行，实得 exit $RC"
elif ! printf '%s' "$OUT" | grep -q 'WARN：HEAD 树'; then
  fail "SKIP_HISTORY=1 放行但未打 WARN（降级必须可见，不得静默）"
else
  printf 'PASS  历史残留 + SKIP_HISTORY=1 → WARN 放行（防提交死锁，降级可见）\n'
fi

# ---------- 11. F1：--root 传子目录 → exit 2（否则静默扫 0 文件） ----------
R=$(new_repo root-subdir)
write_tracked "$R" leak.sh "ADMIN=\"Authorization: Bearer pony_admin_$H32\""
mkdir -p "$R/sub/deep"
OUT=$( cd "$ROOT" && bash "$GATE" --root "$R/sub/deep" 2>&1 ) && RC=0 || RC=$?
if [ "$RC" -eq 0 ]; then
  fail "--root 传子目录应报错（exit 2），实得 exit 0——子目录扫描面为空 = 静默假绿"
elif [ "$RC" -ne 2 ]; then
  fail "--root 传子目录应 exit 2，实得 exit $RC（退出码语义漂移）"
else
  printf 'PASS  --root 传子目录 fail-closed（exit 2，拒绝静默扫 0 文件）\n'
fi

# ---------- 12. 非 git 目录 → exit 2（fail-closed，不当放行） ----------
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

# ---------- 13. 真实仓库（无参模式：索引 + HEAD）→ 放行（端到端同判据） ----------
OUT=$( cd "$ROOT" && bash "$GATE" 2>&1 ) && RC=0 || RC=$?
if [ "$RC" -ne 0 ]; then
  fail "真实仓库无参扫描应为 0，实得 exit $RC（索引/HEAD 仍含凭据形状）"
  printf '%s\n' "$OUT" | tail -n 5 | sed 's/^/      /' >&2
else
  printf 'PASS  真实仓库无参扫描放行（索引 + HEAD，exit 0）\n'
fi

if [ "$FAIL" -ne 0 ]; then
  printf '[%s] 未通过：门的拒绝/放行语义与预期不符（见上方 FAIL）\n' "$SPEC" >&2
  exit 1
fi
printf '[%s] 全部通过：四类凭据被拒（含小写头名）+ 索引/HEAD 双面扫描（暂存净工作区拒 / 工作区脏索引净放 / 历史残留默认拒且可显式降级）+ 占位串与未跟踪放行 + --root 子目录与非 git fail-closed + 真仓放行（exit 0）\n' "$SPEC"
exit 0
