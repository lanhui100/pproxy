#!/usr/bin/env bash
# ============================================================
# ci-verify.sh —— forge 无关的全树门（门禁资产 A4）
#
# 为什么需要它：CI 平台是外壳，门禁逻辑本体必须在本仓内**真实可执行**。
# 于是把"远端层要跑的检查"抽成本脚本：
#   - GitHub 由 10 行适配器 `.github/workflows/ponygo-governance.yml` 调它；
#   - GitLab / Jenkins / Drone / Azure / Bitbucket 在自己的流水线里加一行同样调用即可；
#   - 本地也能直接 `bash .meta/gates/ci-verify.sh` 复现远端结论（不用开 PR 才知道）。
# 平台适配器是薄壳（A5 不进宇宙表），门禁本体在这里（A4 恒生成、进宇宙表）。
#
# 两道：
#   1) 全树 ADR 格式（判定真源 verify-note.sh 无参模式：1.1–1.10，含 1.9 文档有家与 1.10 备选节）
#   2) 暂存区门的负样本 spec（证明 A1 会拒绝非法样本——门禁自证，非产品检查）
#
# 约定：本目录脚本一律 `chmod +x`（非可执行钩子会被 git 静默忽略，判据会假达标）。
# 只读：不写仓库任何文件。
# ============================================================
set -u

ROOT=$(cd "$(dirname "$0")/../.." && pwd) || {
  echo "ci-verify: 无法解析仓库根" >&2
  exit 1
}
cd "$ROOT" || exit 1

fail=0

echo "== [1/2] 全树 ADR 格式（verify-note.sh，1.1–1.10）=="
if ! bash .agents/skills/write-adr/verify-note.sh; then
  echo "ci-verify: FAIL 全树 ADR 格式门未通过 → 逐项修 FAIL 后重跑" >&2
  fail=1
fi

echo "== [2/2] 暂存区门负样本 spec（adr-verify.spec.sh）=="
if ! bash .meta/gates/adr-verify.spec.sh; then
  echo "ci-verify: FAIL 门禁自检未通过（A1 的拒绝/放行语义与预期不符）" >&2
  fail=1
fi

if [ "$fail" -ne 0 ]; then
  echo "ci-verify: 未通过（见上方 FAIL）" >&2
  exit 1
fi
echo "ci-verify: 通过（全树 ADR 格式 + 门禁自检）"
exit 0
