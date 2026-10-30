#!/usr/bin/env bash
# tests/governance/cases/g2_secret_gate.sh — G2-SECRET-GATE
#
# 冻结契约：
#   a) `.meta/gates/secret-scan.sh` 存在；
#   b) `bash .meta/gates/secret-scan.sh` 在干净仓库 exit 0；
#   c) `bash .meta/gates/secret-scan.spec.sh` exit 0（负样本必须被拒）。
#
# 说明：负样本控制的**行为证据**由 spec.sh 自己的断言提供（它的退出码就是"负样本被拒"的
# 物理收据）。本用例额外打印 spec 的结构观测（是否真的调用门禁、是否存在拒绝分支），
# 作为 [INFO] 供人工复核，不参与判定 —— 避免对实现细节过度约束。

SECRET_GATE='.meta/gates/secret-scan.sh'
SECRET_GATE_SPEC='.meta/gates/secret-scan.spec.sh'

case_G2_SECRET_GATE() {
  local rc spec_calls spec_exit1 n_calls n_exit1

  # a) 门禁脚本存在
  if [ ! -f "$REPO_ROOT/$SECRET_GATE" ]; then
    printf '缺 %s：密钥扫描门禁不存在（矩阵 G2 前置件）\n' "$SECRET_GATE"
    return 1
  fi

  # b) 干净仓库必须放行（exit 0）
  run_cmd "$CMD_T" bash "$SECRET_GATE"
  rc="$CAP_RC"
  if [ "$rc" -ne 0 ]; then
    printf 'bash %s 在干净仓库应 exit 0，实得 rc=%s；输出尾行：%s\n' \
      "$SECRET_GATE" "$rc" "$(last_line "$CAP_OUT")"
    return 1
  fi

  # c) 负样本 spec 存在且 exit 0
  if [ ! -f "$REPO_ROOT/$SECRET_GATE_SPEC" ]; then
    printf '%s rc=0，但缺负样本 spec %s（无法证明负样本会被拒）\n' "$SECRET_GATE" "$SECRET_GATE_SPEC"
    return 1
  fi

  # 结构观测（仅 INFO，不参与判定）
  spec_calls="$(grep -c -e 'secret-scan\.sh' "$REPO_ROOT/$SECRET_GATE_SPEC" || true)"
  spec_exit1="$(grep -c -E 'exit[[:space:]]+1' "$REPO_ROOT/$SECRET_GATE_SPEC" || true)"
  n_calls="$(count_lines "$spec_calls")"
  n_exit1="$(count_lines "$spec_exit1")"
  printf '[INFO] %s 结构观测：引用 secret-scan.sh 的 %s 行，含 exit 1 拒绝分支 %s 处（行数 %s）\n' \
    "$SECRET_GATE_SPEC" "${spec_calls:-0}" "${spec_exit1:-0}" \
    "$(wc -l <"$REPO_ROOT/$SECRET_GATE_SPEC" | tr -d ' ')"

  run_cmd "$CMD_T" bash "$SECRET_GATE_SPEC"
  rc="$CAP_RC"
  if [ "$rc" -ne 0 ]; then
    printf 'bash %s 应 exit 0（负样本必须被拒），实得 rc=%s；输出尾行：%s\n' \
      "$SECRET_GATE_SPEC" "$rc" "$(last_line "$CAP_OUT")"
    return 1
  fi

  printf '门禁 rc=0（干净仓库放行）；spec rc=0（负样本被拒；spec 引用门禁 %s 处/拒绝分支 %s 处）\n' \
    "${spec_calls:-0}" "${spec_exit1:-0}"
  return 0
}
