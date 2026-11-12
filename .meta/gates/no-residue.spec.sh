#!/usr/bin/env bash
# ============================================================
# no-residue.spec.sh —— A8（no-residue.sh）的负样本执行判据（门禁资产 A9）
#
# 为什么需要它：一个「永远 exit 0」的残留门等于没有门。本 spec 用 mktemp -d 造**独立**样本仓，
# 把被测门禁复制进去（门按自身所在位置解析仓库根，故不看 cwd、不从 PATH 取同名脚本），
# 逐类断言「该拒的必拒、该放行的必放行」：
#   ① 命中残留 → 必拒，且必须点名命中文件
#   ② 零残留 → 必放行（门不得一律拒绝 = 正控制）
#   ③ 空清单 → 放行并打印「跳过」（无可判定项，不是通过）
#   ④ 清单缺文件 → 放行并打印「跳过」（缺登记 ≠ 有残留）
#   ⑤ 未知参数 / 多余参数 → 必拒（fail-closed，不静默忽略）
#   ⑥ 正则元字符按**字面**匹配（`a.b*c[d]` 命中原文；`a.b` 不命中 `axb`）
#   ⑦ 范围生效（scope 外的命中不计分）
#   ⑧ 清单格式非法（非空行无 TAB）→ 必拒
#   ⑨ 清单登记的字面串不得被清单文件自身命中
#   ⑩ grep 退化路径（NO_RESIDUE_FORCE_GREP=1）与 rg 路径语义一致（命中/放行两个方向）
#   ⑪ 隐藏目录（.agents/ .meta/ .github/ …）同样在检索面内——两条检索路径都必须命中
#      （防「rg 默认跳过隐藏项 → 范围 . 时 .meta/ 与 .agents/ 整片不检索」的假绿回归）
#
# 用法：bash .meta/gates/no-residue.spec.sh
# 退出码：0 = 全部样本符合预期；1 = 有样本不符预期（逐项 [FAIL]）。
# 依赖：bash + coreutils/mktemp + grep（rg 可选——缺 rg 时 A8 走 grep 退化路径）。
# 只读：只在 mktemp 目录里造样本，不写仓库任何文件。
# ============================================================
set -u

HERE=$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd) || {
  printf 'no-residue.spec: 无法解析脚本目录\n' >&2
  exit 1
}
GATE="$HERE/no-residue.sh"
[ -f "$GATE" ] || {
  printf 'no-residue.spec: FAIL 缺被测门禁 %s（A9 失去被测对象 → 判未验证）\n' "$GATE" >&2
  exit 1
}

FAILN=0
ok()  { printf '  [ok]   %s\n' "$*"; }
bad() { printf '  [FAIL] %s\n' "$*" >&2; FAILN=$((FAILN+1)); }

TMP=$(mktemp -d 2>/dev/null) || { printf 'no-residue.spec: FAIL 无法创建样本目录\n' >&2; exit 1; }
trap 'rm -rf "$TMP"' EXIT

CASE=0
new_case() { # → 打印新建的样本仓根（内含可执行的门禁副本）
  # 目录名用 mktemp 生成：本函数在 $( ) 子 shell 里跑，变量计数器不会回传，靠 mktemp 保证每次唯一。
  local d=""
  CASE=$((CASE+1))
  d=$(mktemp -d "$TMP/case.XXXXXX" 2>/dev/null) || return 1
  mkdir -p "$d/.meta/gates" || return 1
  cp "$GATE" "$d/.meta/gates/no-residue.sh" || return 1
  chmod +x "$d/.meta/gates/no-residue.sh" 2>/dev/null || true
  printf '%s' "$d"
}

OUT=""
RC=0
run_gate() { # <样本仓根> [门禁参数…]
  local d="$1"; shift
  if [ "$#" -gt 0 ]; then
    OUT=$( cd "$d" && bash .meta/gates/no-residue.sh "$@" 2>&1 ); RC=$?
  else
    OUT=$( cd "$d" && bash .meta/gates/no-residue.sh 2>&1 ); RC=$?
  fi
}
run_gate_grep() { # <样本仓根>（强制 grep 退化路径）
  local d="$1"
  OUT=$( cd "$d" && NO_RESIDUE_FORCE_GREP=1 bash .meta/gates/no-residue.sh 2>&1 ); RC=$?
}
tail_of() { printf '%s' "$OUT" | tail -n 2 | tr '\n' ' '; }
assert_rc() { # <期望 rc> <描述>
  if [ "$RC" -eq "$1" ]; then ok "$2（exit $RC）"
  else bad "$2（期望 exit $1，实得 $RC；输出尾部：$(tail_of)）"; fi
}
assert_out_has() { # <子串> <描述>
  if printf '%s\n' "$OUT" | grep -qF -- "$1"; then ok "$2"
  else bad "$2（输出未含「$1」；输出尾部：$(tail_of)）"; fi
}
manifest_of() { printf '%s/.meta/gates/no-residue.manifest' "$1"; }

D=""

# ---------- ① 命中残留 → 必拒 ----------
D=$(new_case) || { bad "A9① 样本仓创建失败"; D=""; }
if [ -n "$D" ]; then
  mkdir -p "$D/src"
  printf 'const gone = "PonyGoResidueProbe42";\n' > "$D/src/app.js"
  printf 'PonyGoResidueProbe42\t.\n' > "$(manifest_of "$D")"
  run_gate "$D"
  assert_rc 1 "A9① 命中残留必须拒"
  assert_out_has "src/app.js" "A9① 命中必须点名命中文件"
fi

# ---------- ② 零残留 → 必放行（正控制）----------
D=$(new_case) || bad "A9② 样本仓创建失败"
if [ -n "$D" ]; then
  mkdir -p "$D/src"
  printf 'clean\n' > "$D/src/app.js"
  printf 'PonyGoNeverThere99\t.\n' > "$(manifest_of "$D")"
  run_gate "$D"
  assert_rc 0 "A9② 零残留必须放行（门不得一律拒绝）"
fi

# ---------- ③ 空清单 / 只有注释 → 放行 + 打印跳过 ----------
D=$(new_case) || bad "A9③ 样本仓创建失败"
if [ -n "$D" ]; then
  : > "$(manifest_of "$D")"
  run_gate "$D"
  assert_rc 0 "A9③ 空清单放行（无可判定项）"
  assert_out_has "跳过" "A9③ 空清单必须打印「跳过」（不得冒充通过）"
  printf '# 只有注释，没有登记\n# PonyGoResidueProbe42\n' > "$(manifest_of "$D")"
  run_gate "$D"
  assert_rc 0 "A9③ 只有注释的清单放行"
fi

# ---------- ④ 清单缺文件 → 放行 + 打印跳过 ----------
D=$(new_case) || bad "A9④ 样本仓创建失败"
if [ -n "$D" ]; then
  run_gate "$D"
  assert_rc 0 "A9④ 清单缺文件放行（缺登记 ≠ 有残留）"
  assert_out_has "跳过" "A9④ 清单缺文件必须打印「跳过」"
fi

# ---------- ⑤ 未知参数 / 多余参数 → 必拒 ----------
D=$(new_case) || bad "A9⑤ 样本仓创建失败"
if [ -n "$D" ]; then
  run_gate "$D" --bogus
  assert_rc 1 "A9⑤ 未知参数必须拒（fail-closed）"
  assert_out_has "未知参数" "A9⑤ 未知参数必须点名参数"
  : > "$(manifest_of "$D")"
  run_gate "$D" a.manifest b.manifest
  assert_rc 1 "A9⑤ 多余参数必须拒（不静默忽略后照常判定）"
fi

# ---------- ⑥ 正则元字符按字面匹配 ----------
D=$(new_case) || bad "A9⑥ 样本仓创建失败"
if [ -n "$D" ]; then
  mkdir -p "$D/src"
  printf 'const re = "a.b*c[d]";\n' > "$D/src/rx.js"
  printf 'a.b*c[d]\t.\n' > "$(manifest_of "$D")"
  run_gate "$D"
  assert_rc 1 "A9⑥ 含正则元字符的字面串命中必须拒（不做正则解释）"
  D=$(new_case) || bad "A9⑥ 反向样本创建失败"
  if [ -n "$D" ]; then
    mkdir -p "$D/src"
    printf 'axb\n' > "$D/src/x.js"
    printf 'a.b\t.\n' > "$(manifest_of "$D")"
    run_gate "$D"
    assert_rc 0 "A9⑥ 正则等价但字面不存在 → 必须放行（字面口径）"
  fi
fi

# ---------- ⑦ 范围（scope）生效 ----------
D=$(new_case) || bad "A9⑦ 样本仓创建失败"
if [ -n "$D" ]; then
  mkdir -p "$D/src" "$D/docs"
  printf 'PonyGoScopeProbe7\n' > "$D/docs/note.md"
  printf 'PonyGoScopeProbe7\tsrc\n' > "$(manifest_of "$D")"
  run_gate "$D"
  assert_rc 0 "A9⑦ 范围外命中不计（scope 生效）"
  printf 'PonyGoScopeProbe7\tdocs\n' > "$(manifest_of "$D")"
  run_gate "$D"
  assert_rc 1 "A9⑦ 范围内命中必须拒（正控制）"
fi

# ---------- ⑧ 清单格式非法 → 必拒 ----------
D=$(new_case) || bad "A9⑧ 样本仓创建失败"
if [ -n "$D" ]; then
  printf 'no-tab-line\n' > "$(manifest_of "$D")"
  run_gate "$D"
  assert_rc 1 "A9⑧ 清单行缺 TAB → 必拒（宁拒不猜）"
  assert_out_has "格式非法" "A9⑧ 必须报「格式非法」并指明行号口径"
fi

# ---------- ⑨ 清单自身不得被自己命中 ----------
D=$(new_case) || bad "A9⑨ 样本仓创建失败"
if [ -n "$D" ]; then
  mkdir -p "$D/src"
  printf 'untouched\n' > "$D/src/a.js"
  # 字面串只出现在清单文件自身里 → 全树检索不得把它算成残留
  printf 'PonyGoManifestOnlyProbe5\t.\n' > "$(manifest_of "$D")"
  run_gate "$D"
  assert_rc 0 "A9⑨ 清单文件自身不构成残留命中"
fi

# ---------- ⑩ grep 退化路径与 rg 路径语义一致 ----------
D=$(new_case) || bad "A9⑩ 样本仓创建失败"
if [ -n "$D" ]; then
  mkdir -p "$D/src"
  printf 'const gone = "PonyGoResidueProbe42";\n' > "$D/src/app.js"
  printf 'PonyGoResidueProbe42\t.\n' > "$(manifest_of "$D")"
  run_gate_grep "$D"
  assert_rc 1 "A9⑩ 退化 grep 路径：命中同样必须拒"
  assert_out_has "src/app.js" "A9⑩ 退化路径同样必须点名命中文件"
  D=$(new_case) || bad "A9⑩ 反向样本创建失败"
  if [ -n "$D" ]; then
    mkdir -p "$D/src"
    printf 'clean\n' > "$D/src/app.js"
    printf 'PonyGoNeverThere99\t.\n' > "$(manifest_of "$D")"
    run_gate_grep "$D"
    assert_rc 0 "A9⑩ 退化 grep 路径：零残留同样必须放行"
  fi
fi

# ---------- ⑪ 隐藏目录同样在检索面内（rg 默认跳过隐藏项 = 假绿）----------
D=$(new_case) || bad "A9⑪ 样本仓创建失败"
if [ -n "$D" ]; then
  mkdir -p "$D/.agents/notes"
  # 字面串只出现在隐藏目录里的文件：rg 不带 --hidden 时整个 .agents/ 不被检索 → 假绿
  printf 'ref to PonyGoHiddenProbe8 in a hidden dir\n' > "$D/.agents/notes/probe.txt"
  printf 'PonyGoHiddenProbe8\t.\n' > "$(manifest_of "$D")"
  run_gate "$D"
  assert_rc 1 "A9⑪ 命中只在隐藏目录（.agents/）→ 必须拒（rg 须 --hidden）"
  assert_out_has ".agents/notes/probe.txt" "A9⑪ 必须点名隐藏目录里的命中文件"
  # 退化路径同样必须命中（grep -r 本就含隐藏项——两条路径口径一致）
  run_gate_grep "$D"
  assert_rc 1 "A9⑪ 退化 grep 路径：隐藏目录命中同样必须拒"
  # 反向控制：隐藏目录里零命中 → 仍必须放行（别把 --hidden 写成一律拒绝）
  D=$(new_case) || bad "A9⑪ 反向样本创建失败"
  if [ -n "$D" ]; then
    mkdir -p "$D/.meta/gates"
    printf 'clean hidden file\n' > "$D/.meta/gates/note.txt"
    printf 'PonyGoHiddenNever9\t.\n' > "$(manifest_of "$D")"
    run_gate "$D"
    assert_rc 0 "A9⑪ 隐藏目录内零命中 → 放行（正控制）"
  fi
fi

if [ "$FAILN" -ne 0 ]; then
  printf '== no-residue.spec：未通过（%s 项不符预期——门要么失效要么误拒）==\n' "$FAILN" >&2
  exit 1
fi
printf '== no-residue.spec：通过（负样本必拒 + 正样本放行 + 参数/清单/范围/字面口径全部符合预期）==\n'
exit 0
