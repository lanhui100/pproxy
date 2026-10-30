#!/usr/bin/env bash
# tests/governance/cases/g0_selftest.sh — G0-SELFTEST（冻结矩阵第一条：防空转假绿）
#
# 契约：构造「假违规样本」调用探测函数，必须返回非零。
# 目的：证明本套件的探测器不是 no-op、记账层不会把非零吞成 PASS、
#       REQUIRED_CASES 兜底真的会拦住"未执行"的用例。
# 判据只依赖探测器自身逻辑（夹具全部落在 $TMPROOT，仓库外），
# 因此任何红相下 G0 仍应 PASS —— G0 红了说明**验收套件本身不可信**，必须先修 harness。
#
# 说明：密钥夹具字面量在运行期拼接生成（`pony_admin_` + %016x），
#      源码中不存在可被 G1/G2 扫描命中的完整密钥，避免验收套件自我污染。

G0_TOTAL=0
G0_FAIL=0
G0_MSG=""

g0_fake_key() { printf 'pony_admin_%016x' "$1"; }

# g0_mkrepo <dir> <tracked-file-content> — 造一个只有 index（无提交）的 git 仓库
g0_mkrepo() {
  local d="$1" content="$2"
  rm -rf "$d"
  mkdir -p "$d" || return 1
  git -C "$d" init -q >/dev/null 2>&1 || return 1
  printf '%s\n' "$content" >"$d/tracked.txt" || return 1
  git -C "$d" add -A >/dev/null 2>&1 || return 1
  return 0
}

# g0_expect <label> <zero|nonzero> <cmd...> — 断言探测函数返回值的"方向"
g0_expect() {
  local label="$1" want="$2" rc
  shift 2
  G0_TOTAL=$((G0_TOTAL + 1))
  "$@" >/dev/null 2>&1
  rc=$?
  if { [ "$want" = "zero" ] && [ "$rc" -eq 0 ]; } || { [ "$want" = "nonzero" ] && [ "$rc" -ne 0 ]; }; then
    return 0
  fi
  G0_FAIL=$((G0_FAIL + 1))
  G0_MSG="$G0_MSG；$label 实测 rc=$rc（期望 $want）"
  return 0
}

case_G0_SELFTEST() {
  G0_TOTAL=0
  G0_FAIL=0
  G0_MSG=""

  local root="$TMPROOT/g0"
  mkdir -p "$root" || {
    printf '无法创建临时夹具目录 %s\n' "$root"
    return 1
  }

  # 1) 密钥门禁探测（复用 G1 的真实探测器）：假违规样本必须被判违规（非零）
  if ! g0_mkrepo "$root/repo-leak" "admin_token=$(g0_fake_key 2143304101)"; then
    printf '夹具 git 仓库创建失败（git 不可用？root=%s）\n' "$root/repo-leak"
    return 1
  fi
  if ! g0_mkrepo "$root/repo-clean" "admin_token=<redacted>"; then
    printf '夹具 git 仓库创建失败（git 不可用？root=%s）\n' "$root/repo-clean"
    return 1
  fi
  g0_expect "密钥探测·假违规样本必须被判违规" nonzero probe_git_no_tracked_secret "$root/repo-leak"
  # 反向控制：干净样本必须放行，否则探测函数是"恒拒绝"的空转，同样不可信
  g0_expect "密钥探测·干净样本必须放行" zero probe_git_no_tracked_secret "$root/repo-clean"

  # 2) 本地哨兵探测（复用 G4 的真实探测器）
  local hook_bad="$root/pre-commit-missing-end" hook_ok="$root/pre-commit-paired"
  printf '#!/usr/bin/env bash\n# %s\n' "$SENTINEL_BEGIN" >"$hook_bad"
  printf '#!/usr/bin/env bash\n# %s\nbash .meta/gates/adr-verify.sh\n# %s\n' \
    "$SENTINEL_BEGIN" "$SENTINEL_END" >"$hook_ok"
  g0_expect "哨兵探测·缺尾哨兵必须被判违规" nonzero probe_sentinel_pair "$hook_bad"
  g0_expect "哨兵探测·哨兵成对必须放行" zero probe_sentinel_pair "$hook_ok"

  # 3) 决策记录探测（复用 G3 的真实探测器）：喂一个恒 exit 0 的空转校验器，
  #    探测器必须察觉"缺节样本未被拒"并判违规 —— 这是"门禁被换成 no-op"这类假绿的直接防线。
  local shadow="$root/shadow" stub_noop="$root/stub-noop.sh" stub_real="$root/stub-real.sh"
  mkdir -p "$shadow/.agents/notes/proposed/process" "$root/bin" || {
    printf '无法创建影子决策记录树 %s\n' "$shadow"
    return 1
  }
  printf '# note\nStatus: proposed\n\n## Proposal\nx\n' \
    >"$shadow/.agents/notes/proposed/process/2026-10-30-g0-bad.md"
  printf '# note\nStatus: proposed\n\n## Proposal\nx\n\n## Alternatives considered\ny\n' \
    >"$shadow/.agents/notes/proposed/process/2026-10-30-g0-good.md"
  printf '#!/usr/bin/env bash\nexit 0\n' >"$stub_noop"
  printf '#!/usr/bin/env bash\ncase "$1" in *g0-bad.md) exit 1 ;; *) exit 0 ;; esac\n' >"$stub_real"
  g0_expect "note 探测·空转校验器必须被判违规" nonzero \
    probe_note_section_enforced "$stub_noop" "$shadow" \
    ".agents/notes/proposed/process/2026-10-30-g0-bad.md" \
    ".agents/notes/proposed/process/2026-10-30-g0-good.md"
  # 反向控制：喂一个"正例放行、反例拒绝"的最小正确校验器，探测器必须放行
  g0_expect "note 探测·最小正确校验器必须放行" zero \
    probe_note_section_enforced "$stub_real" "$shadow" \
    ".agents/notes/proposed/process/2026-10-30-g0-bad.md" \
    ".agents/notes/proposed/process/2026-10-30-g0-good.md"

  # 4) 记账层自检：注入一条 rc=1 的假判定，必须真的产生 [FAIL] 与 FAIL_N=1（不得恒 PASS）
  G0_TOTAL=$((G0_TOTAL + 1))
  local acct
  acct="$( record __G0_INJECTED__ 1 "注入的假 FAIL（仅子 shell 内，不进最终账本）" 2>&1; printf '\nFAIL_N=%s PASS_N=%s' "$FAIL_N" "$PASS_N" )"
  if [[ "$acct" == "[FAIL] __G0_INJECTED__ "*"FAIL_N=1 PASS_N=0" ]]; then
    : # 记账层健康
  else
    G0_FAIL=$((G0_FAIL + 1))
    G0_MSG="$G0_MSG；记账层自检失败（注入 rc=1 未产生 FAIL 记账：$(oneline "$acct")）"
  fi

  # 5) MISSING-CASE 兜底自检：一个从未执行的 case-id 必须被判"未执行"
  G0_TOTAL=$((G0_TOTAL + 1))
  if executed "__G0_NEVER_EXECUTED__"; then
    G0_FAIL=$((G0_FAIL + 1))
    G0_MSG="$G0_MSG；MISSING-CASE 兜底自检失败（未执行 id 被当成已执行，漏跑会被放行）"
  fi

  if [ "$G0_FAIL" -eq 0 ]; then
    printf '自检 %d/%d 通过：探测器对假违规样本均返回非零、对合规样本返回 0；记账层与 MISSING-CASE 兜底无假绿\n' \
      "$G0_TOTAL" "$G0_TOTAL"
    return 0
  fi
  printf '自检 %d/%d 通过；失败项：%s\n' \
    "$((G0_TOTAL - G0_FAIL))" "$G0_TOTAL" "${G0_MSG#；}"
  return 1
}
