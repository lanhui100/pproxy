#!/usr/bin/env bash
# tests/governance/run.sh — pproxy「治理修复」独立黑盒验收入口（L2-T 独立 Test Agent）。
#
# 合同唯一依据：Lead 冻结的 G0..G9 契约矩阵（见 tests/governance/README.md）
# 本文件只做编排：用例函数全部在 cases/ 下，一条用例一个独立函数，禁止互相兜底。
#
# 用法（全部 cwd=/home/dm/pproxy）：
#   bash tests/governance/run.sh              # 全量 + 必需用例完备校验
#   bash tests/governance/run.sh '^G3'        # 仅跑匹配正则的用例（调试用；不做完备校验）
#   bash tests/governance/run.sh --list       # 只列出冻结用例 id
#   GOV_KEEP=1 bash tests/governance/run.sh   # 保留临时夹具目录（排障）
#
# 环境变量：
#   GOV_CMD_T      单条外部命令超时秒（缺省 90），超时按违规处理
#   GOV_TOTAL_T    整套全局时间预算秒（缺省 600），耗尽后剩余必需用例一律判 FAIL
#
# 输出协议（机器可读，每行一条判定）：
#   [PASS] <case-id> <实测 detail>
#   [FAIL] <case-id> <实测 detail>
#   [INFO] <纯观测，不参与判定>
#   末行：ACCEPT_EXIT=<0|1>，进程退出码与之一致。
#
# 反假绿纪律：
#   - 必备用例（REQUIRED_CASES）未被真实执行 → MISSING-CASE 兜底判 FAIL（漏跑不得放行）。
#   - 用例函数未定义/未加载 → 该用例判 FAIL（不允许静默跳过）。
#   - 不提供 SKIP：无法验证即 FAIL。
#   - G0-SELFTEST 证明探测器与记账层不是空转（见 cases/g0_selftest.sh）。
#   - 全程零网络依赖；全部夹具落在仓库外临时目录，验收不修改被验收仓库。
set -uo pipefail

GOV_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
# shellcheck source=lib.sh
source "$GOV_DIR/lib.sh"

TOTAL_T="${GOV_TOTAL_T:-600}"
START_S="$(date +%s)"
TMPROOT=""

# 冻结用例集（顺序即执行顺序）。任何 id 缺失都不得放行。
REQUIRED_CASES="G0-SELFTEST G1-NO-TRACKED-SECRETS G2-SECRET-GATE G3-NOTE-1.10 G4-SENTINEL-LOCAL G5-REMOTE-GATE G6-ADAPTATION G7-IMPLEMENTED-REPRODUCIBLE G8-DOCS G9-HYGIENE"

CASE_TABLE=(
  "G0-SELFTEST|case_G0_SELFTEST"
  "G1-NO-TRACKED-SECRETS|case_G1_NO_TRACKED_SECRETS"
  "G2-SECRET-GATE|case_G2_SECRET_GATE"
  "G3-NOTE-1.10|case_G3_NOTE_110"
  "G4-SENTINEL-LOCAL|case_G4_SENTINEL_LOCAL"
  "G5-REMOTE-GATE|case_G5_REMOTE_GATE"
  "G6-ADAPTATION|case_G6_ADAPTATION"
  "G7-IMPLEMENTED-REPRODUCIBLE|case_G7_IMPLEMENTED_REPRODUCIBLE"
  "G8-DOCS|case_G8_DOCS"
  "G9-HYGIENE|case_G9_HYGIENE"
)

cleanup() {
  local rc=$?
  if [ -n "$TMPROOT" ] && [ -d "$TMPROOT" ]; then
    if [ "${GOV_KEEP:-0}" = "1" ]; then
      printf '[INFO] GOV_KEEP=1，临时夹具目录保留：%s\n' "$TMPROOT"
    else
      rm -rf "$TMPROOT"
    fi
  fi
  return $rc
}
trap cleanup EXIT INT TERM

elapsed() { printf '%s' "$(( $(date +%s) - START_S ))"; }

summary() {
  local missing="" id e
  if [ -z "$FILTER" ]; then
    for id in $REQUIRED_CASES; do
      executed "$id" || missing="$missing $id"
    done
    if [ -n "$missing" ]; then
      # 漏跑兜底：没被执行 = 没证据 = FAIL（不是 SKIP、不是默认通过）
      record MISSING-CASE 1 "必需用例未被执行（fail-closed 兜底，禁止漏跑放行）:${missing}"
    fi
  fi

  e="$(elapsed)"
  printf '\n=== GOVERNANCE ACCEPT SUMMARY ===\n'
  printf 'REPO=%s\n' "$REPO_ROOT"
  printf 'HEAD=%s\n' "$(git -C "$REPO_ROOT" rev-parse --short HEAD 2>/dev/null || printf 'no-git')"
  printf 'ELAPSED=%ss CMD_T=%ss TOTAL_T=%ss\n' "$e" "$CMD_T" "$TOTAL_T"
  printf 'PASS=%d FAIL=%d\n' "$PASS_N" "$FAIL_N"
  if [ -n "$FAILED_IDS" ]; then printf 'FAILED_IDS=%s\n' "$FAILED_IDS"; fi
  if [ "$FAIL_N" -eq 0 ]; then
    printf 'ACCEPT_EXIT=0\n'
    return 0
  fi
  printf 'ACCEPT_EXIT=1\n'
  return 1
}

usage() {
  printf '用法: bash tests/governance/run.sh [--list] [<case-id 正则>]\n'
}

main() {
  FILTER=""
  case "${1:-}" in
    --list)
      local entry
      for entry in "${CASE_TABLE[@]}"; do printf '%s\n' "${entry%%|*}"; done
      return 0
      ;;
    -h | --help)
      usage
      return 0
      ;;
    "") : ;;
    *)
      FILTER="$1"
      ;;
  esac

  # ponygo 可能装在 ~/.local/bin（G4/G6 依赖）；缺了才补，不覆盖既有 PATH 顺序
  if ! command -v ponygo >/dev/null 2>&1 && [ -x "${HOME:-/root}/.local/bin/ponygo" ]; then
    PATH="${HOME:-/root}/.local/bin:$PATH"
    export PATH
  fi

  TMPROOT="$(mktemp -d "${TMPDIR:-/tmp}/pproxy-gov.XXXXXX")" || {
    printf '致命：无法创建临时夹具目录（mktemp 失败）\n'
    printf 'ACCEPT_EXIT=1\n'
    return 1
  }

  cd "$REPO_ROOT" || {
    printf '致命：无法进入仓库根 %s\n' "$REPO_ROOT"
    printf 'ACCEPT_EXIT=1\n'
    return 1
  }

  printf '=== GOVERNANCE ACCEPT RUN ===\n'
  printf '[INFO] REPO_ROOT=%s (cwd=%s)\n' "$REPO_ROOT" "$(pwd)"
  printf '[INFO] HEAD=%s branch=%s\n' \
    "$(git rev-parse --short HEAD 2>/dev/null || printf 'no-git')" \
    "$(git rev-parse --abbrev-ref HEAD 2>/dev/null || printf 'no-git')"
  printf '[INFO] TMPROOT=%s CMD_T=%ss TOTAL_T=%ss bash=%s\n' \
    "$TMPROOT" "$CMD_T" "$TOTAL_T" "${BASH_VERSION}"
  printf '[INFO] ponygo=%s\n' "$(command -v ponygo 2>/dev/null || printf 'NOT-FOUND')"
  if ! git rev-parse --is-inside-work-tree >/dev/null 2>&1; then
    printf '[INFO] WARN 当前目录不是 git 工作树：依赖 git 的用例将如实报红\n'
  fi
  if [ -n "$FILTER" ]; then
    note "FILTER='$FILTER'：仅运行匹配用例（不做 REQUIRED_CASES 完备校验，结果不可用于验收）"
  fi

  # 用例文件在 TMPROOT 就绪后 source（用例常量引用 $TMPROOT）
  # shellcheck source=cases/g0_selftest.sh
  source "$GOV_DIR/cases/g0_selftest.sh"
  # shellcheck source=cases/g1_no_tracked_secrets.sh
  source "$GOV_DIR/cases/g1_no_tracked_secrets.sh"
  # shellcheck source=cases/g2_secret_gate.sh
  source "$GOV_DIR/cases/g2_secret_gate.sh"
  # shellcheck source=cases/g3_note_110.sh
  source "$GOV_DIR/cases/g3_note_110.sh"
  # shellcheck source=cases/g4_sentinel_local.sh
  source "$GOV_DIR/cases/g4_sentinel_local.sh"
  # shellcheck source=cases/g5_remote_gate.sh
  source "$GOV_DIR/cases/g5_remote_gate.sh"
  # shellcheck source=cases/g6_adaptation.sh
  source "$GOV_DIR/cases/g6_adaptation.sh"
  # shellcheck source=cases/g7_implemented_reproducible.sh
  source "$GOV_DIR/cases/g7_implemented_reproducible.sh"
  # shellcheck source=cases/g8_docs.sh
  source "$GOV_DIR/cases/g8_docs.sh"
  # shellcheck source=cases/g9_hygiene.sh
  source "$GOV_DIR/cases/g9_hygiene.sh"

  local entry id fn
  for entry in "${CASE_TABLE[@]}"; do
    id="${entry%%|*}"
    fn="${entry##*|}"
    if [ -n "$FILTER" ] && ! [[ "$id" =~ $FILTER ]]; then continue; fi

    # 全局预算保护：预算耗尽后不再静默跳过，如实判 FAIL（无法验证 ≠ 通过）
    if [ "$(elapsed)" -ge "$TOTAL_T" ]; then
      record "$id" 1 "全局时间预算 ${TOTAL_T}s 耗尽，用例未执行（fail-closed）"
      continue
    fi
    # 用例函数必须真实存在于本次加载中，否则不得放行
    if ! declare -F "$fn" >/dev/null 2>&1; then
      record "$id" 1 "用例函数 $fn 未定义（cases/ 未加载或命名漂移，harness 缺陷）"
      continue
    fi
    run_probe "$id" "$fn"
  done

  if summary; then return 0; else return 1; fi
}

main "$@"
