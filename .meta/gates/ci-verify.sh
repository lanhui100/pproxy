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
# 四段：
#   1) 全树 ADR 格式（判定真源 verify-note.sh 无参模式：1.1–1.11，含 1.9 文档有家、1.10 备选节
#      与 1.11 负空间形状 / 重引入条件）
#   2) 暂存区门的负样本 spec（证明 A1 会拒绝非法样本——门禁自证，非产品检查）
#   3) 归档 SHA-256 封印（A6 check + A7 负样本自检）——缺 A6/A7 时 **fail-closed**（exit 1），
#      不静默跳过：全树兜底门缺件 = 封印无人看守，按「未验证不得计为 PASS」处理。
#   4) 引用残留（A8 no-residue.sh + A9 自检）——**缺件不失败**（A8/A9 属实例登记类门：清单不存在
#      = 无可判定项，门自己打印跳过并 exit 0），但文件在场就按真实退出码判，不得「在场但跳过」。
#
# 参数：本脚本零旗标（runner 直接调）；**未知参数一律 exit 1**（fail-closed）——静默忽略参数会让
#       `bash ci-verify.sh --strict` 这类误用变成「跑了、全绿」的假证据。`-h|--help` 打印用法后 exit 0。
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

for a in "$@"; do
  case "$a" in
    -h|--help)
      printf '%s\n' '用法：bash .meta/gates/ci-verify.sh    （零旗标；四段：全树 ADR 格式 / 暂存区门自检 / 归档封印 / 引用残留）' \
        '退出码：0 全部通过；1 任一段 FAIL 或参数错误。'
      exit 0 ;;
    *)
      printf 'ci-verify: FAIL 未知参数：%s（本脚本零旗标；--help 看用法）\n' "$a" >&2
      exit 1 ;;
  esac
done

fail=0

echo "== [1/4] 全树 ADR 格式（verify-note.sh，1.1–1.11）=="
if ! bash .agents/skills/write-adr/verify-note.sh; then
  echo "ci-verify: FAIL 全树 ADR 格式门未通过 → 逐项修 FAIL 后重跑" >&2
  fail=1
fi

echo "== [2/4] 暂存区门负样本 spec（adr-verify.spec.sh）=="
# 分开捕获真实退出码与输出：SKIPPED + 0 同样是未验证，不得被 ! 改写后洗成通过。
spec_out=$(bash .meta/gates/adr-verify.spec.sh 2>&1); spec_rc=$?
printf '%s\n' "$spec_out"
if [ "$spec_rc" -ne 0 ] || ! printf '%s\n' "$spec_out" | grep -qF '[adr-verify.spec] 全部通过'; then
  echo "ci-verify: FAIL 门禁自检未通过（exit $spec_rc；缺 A2 精确完成标记即未验证）" >&2
  fail=1
fi

echo "== [3/4] 归档封印（archive-verify.sh check + archive-verify.spec.sh）=="
if [ ! -f .meta/gates/archive-verify.sh ]; then
  echo "ci-verify: FAIL 第 3 段缺资产 .meta/gates/archive-verify.sh（fail-closed：缺件不得静默跳过）→ 跑 'ponygo init --repair-skeleton' 补齐" >&2
  fail=1
elif ! bash .meta/gates/archive-verify.sh check; then
  echo "ci-verify: FAIL 归档封印未通过（归档件被改动或未封印）→ 恢复原字节，或按 append-only 跑架构封印追加（archive-verify.sh seal）" >&2
  fail=1
fi
if [ ! -f .meta/gates/archive-verify.spec.sh ]; then
  echo "ci-verify: FAIL 第 3 段缺资产 .meta/gates/archive-verify.spec.sh（fail-closed：缺件不得静默跳过）→ 跑 'ponygo init --repair-skeleton' 补齐" >&2
  fail=1
else
  archive_spec_out=$(bash .meta/gates/archive-verify.spec.sh 2>&1); archive_spec_rc=$?
  printf '%s\n' "$archive_spec_out"
  if [ "$archive_spec_rc" -ne 0 ] || ! printf '%s\n' "$archive_spec_out" | grep -qF '== archive-verify.spec：通过（5 类负样本 + 1 合法样本全部符合预期）=='; then
    echo "ci-verify: FAIL 归档封印门自检未通过（exit $archive_spec_rc；缺 A7 精确完成标记即未验证）" >&2
    fail=1
  fi
fi

echo "== [4/4] 引用残留（no-residue.sh + no-residue.spec.sh）=="
# 缺件只提示、不失败：A8/A9 属「实例登记」类门（登记什么被删名字由人写清单），
# 与 A6/A7 的「封印无人看守即安全洞」不同类——没登记不等于有残留。
if [ -f .meta/gates/no-residue.sh ]; then
  if ! bash .meta/gates/no-residue.sh; then
    echo "ci-verify: FAIL 引用残留门未通过（清单登记的被删名字仍能被检索到）→ 清残留，或确认无需再守后移除该条登记" >&2
    fail=1
  fi
else
  echo "ci-verify: 提示 第 4 段缺资产 .meta/gates/no-residue.sh（不失败：A8 属实例登记类门；需要时跑 'ponygo init --repair-skeleton' 补齐）"
fi
if [ -f .meta/gates/no-residue.spec.sh ]; then
  nr_spec_out=$(bash .meta/gates/no-residue.spec.sh 2>&1); nr_spec_rc=$?
  printf '%s\n' "$nr_spec_out"
  if [ "$nr_spec_rc" -ne 0 ] || ! printf '%s\n' "$nr_spec_out" | grep -qF '== no-residue.spec：通过'; then
    echo "ci-verify: FAIL 残留门自检未通过（exit $nr_spec_rc；缺 A9 精确完成标记即未验证）" >&2
    fail=1
  fi
else
  echo "ci-verify: 提示 第 4 段缺资产 .meta/gates/no-residue.spec.sh（不失败：A8/A9 属实例登记类门；需要时跑 'ponygo init --repair-skeleton' 补齐）"
fi

if [ "$fail" -ne 0 ]; then
  echo "ci-verify: 未通过（见上方 FAIL）" >&2
  exit 1
fi
echo "ci-verify: 通过（四段：全树 ADR 格式 + 暂存区门自检 + 归档封印 + 引用残留）"
exit 0
