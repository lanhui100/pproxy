#!/usr/bin/env bash
# ============================================================
# no-residue.sh —— 引用残留门（门禁资产 A8）
#
# 用途：删符号/删文件/删目录之后，用「被删名字的字面串」扫全树，证明「搜索零残留」
#       （常载命约 3：删代码前先搜消费者，验收 = 搜索零残留 + 门禁照常绿）。
#       门禁只做**判定**；登记什么名字、范围多宽，由人写在清单里。
#
# 用法：
#   bash .meta/gates/no-residue.sh                # 默认清单 .meta/gates/no-residue.manifest
#   bash .meta/gates/no-residue.sh <清单路径>     # 指定清单（CI / 夹具 / 多清单）
#   bash .meta/gates/no-residue.sh --help         # 打印本用法
#
# 清单格式（UTF-8，每行一条，TAB 分隔）：
#   <被删名字的字面串><TAB><检索范围>
#   - 字面串 = 固定字符串（**不做**正则解释）；首个 TAB 之前为字面串，其后整段为范围；
#   - 范围 = 相对仓库根的路径（目录或文件）；`.` = 全树；范围不存在 = 该条无可检索面（不判）；
#   - 空行与 `#` 开头的行忽略；非空且无 TAB 的行 = 格式非法 → exit 1（fail-closed）。
#
# 退出语义：
#   0  零残留；或「无可判定项」——清单不存在 / 清单为空（含只有注释）→ 打印「跳过」，不判定；
#   1  命中残留（逐条列出「字面串 ← 命中文件」）；清单格式非法；参数错误（未知参数 / 多余参数）。
#
# 判定口径（固定字符串，字面匹配）：
#   rg --fixed-strings --hidden --no-messages -l -e <字面串> -- <范围>
#   （`--hidden` + 只排除 .git：检索面 = 当前树的**全部**文件，含 .meta/ .agents/ .github/ 等
#     隐藏目录；rg 默认跳过隐藏项，漏了它就是假绿——删掉的符号若只在隐藏目录被引用，门会说"零残留"）
#   本框架的运行时承诺只有 bash，rg 不是保证存在的依赖 → 无 rg 时退化为
#   grep -rIlF -e <字面串> -- <范围>（同样字面匹配、列文件名、抑制杂音），并在 stderr 声明退化，
#   不假装「有 rg 的精确度」。两者都：排除 .git/；排除清单文件自身（清单里写着字面串，否则自命中）。
# 已知边界（诚实条款）：rg 默认尊重 .gitignore/.ignore，grep 退化路径不尊重——被 ignore 的文件在
#   退化路径下仍会被检索（更严，不放宽）；跨行字符串（一个字面串被换行拆开）两边都查不到。
#   隐藏目录/文件的检索面**两条分支一致**：rg 分支靠 `--hidden` 纳入（否则 .meta/ .agents/ .github/
#   会被整片跳过 = 假绿），grep 分支的 `grep -r` 本来就含隐藏项；两边都只排除 .git/。
# 测试接缝：NO_RESIDUE_FORCE_GREP=1 强制走退化路径（仅 A9 自检用，正常运行不需要设它）。
#
# 约定：本目录脚本一律 `chmod +x`（非可执行门禁会被静默跳过，判据会假达标）。
# 只读：不写仓库任何文件。
# ============================================================
set -u

usage() {
  printf '%s\n' \
    '用法：bash .meta/gates/no-residue.sh [清单路径]' \
    '  （无参）      默认清单 = .meta/gates/no-residue.manifest' \
    '  <清单路径>    指定清单文件' \
    '  --help        打印用法' \
    '' \
    '清单格式（每行一条，TAB 分隔）：<被删名字的字面串><TAB><检索范围>' \
    '退出码：0 零残留/无可判定项；1 命中残留、清单格式非法或参数错误。'
}

ROOT=$(cd "$(dirname "$0")/../.." && pwd) || {
  printf 'no-residue: 无法解析仓库根\n' >&2
  exit 1
}
cd "$ROOT" || exit 1

MANIFEST=".meta/gates/no-residue.manifest"
seen_pos=0
for a in "$@"; do
  case "$a" in
    -h|--help) usage; exit 0 ;;
    -*) printf 'no-residue: 未知参数：%s（用法见 --help）\n' "$a" >&2; exit 1 ;;
    *)
      if [ "$seen_pos" -eq 1 ]; then
        printf 'no-residue: 多余参数：%s（只接受一个清单路径）\n' "$a" >&2
        exit 1
      fi
      MANIFEST="$a"; seen_pos=1 ;;
  esac
done

if [ ! -f "$MANIFEST" ]; then
  printf 'no-residue: 跳过（清单不存在：%s —— 无被删名字登记 = 无可判定项）\n' "$MANIFEST"
  exit 0
fi

# 清单路径归一为「相对仓库根」，用于把清单自身从命中里剔掉（绝对路径 → 相对；仓外 → 原样，不命中）
MANIFEST_REL="$MANIFEST"
case "$MANIFEST_REL" in
  "$ROOT"/*) MANIFEST_REL="${MANIFEST_REL#"$ROOT"/}" ;;
esac
MANIFEST_REL="${MANIFEST_REL#./}"

HAS_RG=0
command -v rg >/dev/null 2>&1 && HAS_RG=1
# 测试接缝（A9 用）：强制走 grep 退化路径，证明「有 rg / 无 rg」两条检索路径语义一致
# （否则退化路径只在没装 rg 的机器上被跑过，等于没看守）。
[ "${NO_RESIDUE_FORCE_GREP:-0}" = "1" ] && HAS_RG=0

n=0 hits=0 bad=0 lineno=0 noted_fallback=0 pat="" scope="" found="" line=""
while IFS= read -r line || [ -n "$line" ]; do
  lineno=$((lineno+1))
  line="${line%$'\r'}"
  case "$line" in ''|'#'*) continue ;; esac
  case "$line" in
    *$'\t'*) pat="${line%%$'\t'*}"; scope="${line#*$'\t'}" ;;
    *)
      printf 'no-residue: 清单第 %s 行格式非法（应为 <字面串>TAB<范围>）：%s\n' "$lineno" "$line" >&2
      bad=1; continue ;;
  esac
  if [ -z "$pat" ]; then
    printf 'no-residue: 清单第 %s 行字面串为空（空串会命中一切）→ 拒绝\n' "$lineno" >&2
    bad=1; continue
  fi
  [ -n "$scope" ] || scope="."
  n=$((n+1))
  if [ "$HAS_RG" = "0" ] && [ "$noted_fallback" = "0" ]; then
    printf 'no-residue: 注：rg 不可用 → 退化 grep -F（字面匹配语义等价；不尊重 .gitignore，更严）\n' >&2
    noted_fallback=1
  fi
  found=""
  if [ "$HAS_RG" = "1" ]; then
    # `--hidden` 必须有：rg 默认跳过隐藏文件/目录，范围 `.` 时 `.meta/`、`.agents/`、`.github/`
    # 会整个不被检索 → 「删掉的符号只在隐藏目录被引用」时门报零残留 = 假绿（真机复现过）。
    # `.git/` 不是检索面（历史不是当前树），显式排除；清单自身也排除（否则清单里的字面串自命中）。
    found=$(rg --fixed-strings --hidden --no-messages -l \
      --glob '!.git' --glob '!.git/**' --glob "!$MANIFEST_REL" \
      -e "$pat" -- "$scope" 2>/dev/null || true)
  else
    found=$(grep -rIlF --exclude-dir=.git -e "$pat" -- "$scope" 2>/dev/null || true)
  fi
  # 路径归一（去掉 rg 的 ./ 前缀）后剔除清单自身：清单里登记的字面串不该被自己命中
  if [ -n "$found" ]; then
    found=$(printf '%s\n' "$found" | sed 's|^\./||' | grep -vxF -e "$MANIFEST_REL" || true)
  fi
  if [ -n "$found" ]; then
    hits=$((hits+1))
    printf 'no-residue: 残留命中「%s」← %s\n' "$pat" "$(printf '%s' "$found" | tr '\n' ' ')"
  fi
done < "$MANIFEST"

if [ "$bad" -ne 0 ]; then
  printf 'no-residue: FAIL 清单格式非法（见上）——宁拒不猜，修好清单再跑\n' >&2
  exit 1
fi
if [ "$n" -eq 0 ]; then
  printf 'no-residue: 跳过（清单为空/只有注释：%s —— 无可判定项）\n' "$MANIFEST"
  exit 0
fi
if [ "$hits" -ne 0 ]; then
  printf 'no-residue: FAIL %s/%s 条登记仍有残留（删干净 = 搜索零残留；修法：清掉残留，或确认无需再守后移除该条登记）\n' \
    "$hits" "$n" >&2
  exit 1
fi
printf 'no-residue: 通过（%s 条登记的被删名字全树零残留）\n' "$n"
exit 0
