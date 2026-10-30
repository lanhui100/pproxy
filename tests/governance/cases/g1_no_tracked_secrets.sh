#!/usr/bin/env bash
# tests/governance/cases/g1_no_tracked_secrets.sh — G1-NO-TRACKED-SECRETS
#
# 冻结契约：`git grep -lE 'pony_admin_[0-9a-f]{16,}|sk-pony-[0-9a-f]{16,}'` 无输出 → PASS。
# 语义：git 索引+工作树范围内（即被追踪文件）不得命中密钥正则。
#
# 退出码映射（git grep 原生语义，逐档显式处理，禁止一律当违规或一律当合规）：
#   0 → 有命中 → 违规（列出文件）
#   1 → 无命中 → 合规
#   其它（2/124/128…）→ 无法判定 → 违规（fail-closed：不可验证不等于通过）

# 契约字面正则（唯一真源；G0 的夹具断言也走同一常量，防止"正则被改空"这类假绿）
SECRET_RE='pony_admin_[0-9a-f]{16,}|sk-pony-[0-9a-f]{16,}'

# probe_git_no_tracked_secret [<repo-dir>] — 0=合规（0 命中）
probe_git_no_tracked_secret() {
  local dir="${1:-$REPO_ROOT}" rc n
  run_cmd "$CMD_T" git -C "$dir" -c core.quotepath=false grep -lE "$SECRET_RE"
  rc="$CAP_RC"
  case "$rc" in
    0)
      n="$(count_lines "$CAP_OUT")"
      printf 'git grep -lE 命中 %s 个追踪文件（期望 0）：%s\n' "$n" "$(list_head "$CAP_OUT" 6)"
      return 1
      ;;
    1)
      if [ -n "$(printf '%s' "$CAP_OUT" | tr -d '[:space:]')" ]; then
        printf 'git grep rc=1 但仍有输出，语义异常：%s\n' "$(oneline "$CAP_OUT")"
        return 1
      fi
      printf 'git grep -lE 无输出：追踪文件 0 命中（repo=%s）\n' "$dir"
      return 0
      ;;
    *)
      printf 'git grep 无法判定（rc=%s，repo=%s，预算=%ss）：%s\n' \
        "$rc" "$dir" "$CMD_T" "$(oneline "$CAP_OUT")"
      return 1
      ;;
  esac
}

case_G1_NO_TRACKED_SECRETS() {
  probe_git_no_tracked_secret "$REPO_ROOT"
}
