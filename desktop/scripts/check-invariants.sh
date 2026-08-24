#!/usr/bin/env bash
# 不变量门禁（M7 SPEC §6.3）：契约层零漂移；src-tauri 仅白名单例外可动。
# SEC-4 加固：检查②③由「diff 行级白名单 grep」改为 jq 语义比对——行内合并字段
# （如 `+ "title": "x", "withGlobalTauri": true,`）不再能借白名单行蒙混过关。
# 用法: bash scripts/check-invariants.sh <base-ref>   （如 HEAD、origin/master）
# 退出码: 0=通过 1=存在越界改动 2=基线 ref 不可解析 3=缺 jq
set -euo pipefail
BASE="${1:?usage: check-invariants.sh <base-ref>}"

command -v jq >/dev/null || { echo "[invariants] 需要 jq 但未安装"; exit 3; }

DESKTOP="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
GITR=(git -C "$DESKTOP")

# 基线必须可解析，防止拼错的 ref 静默放行
"${GITR[@]}" rev-parse --verify --quiet "$BASE^{commit}" >/dev/null \
  || { echo "[invariants] 基线 ref 不可解析: $BASE"; exit 2; }

REPO_ROOT="$("${GITR[@]}" rev-parse --show-toplevel)"   # worktree 场景取本工作树根
CONF_WORK="$DESKTOP/src-tauri/tauri.conf.json"
CAP_WORK="$DESKTOP/src-tauri/capabilities/default.json"

TMPD="$(mktemp -d)"
trap 'rm -rf "$TMPD"' EXIT

fail=0

# JSON 解析失败本身即违规（防手改坏文件后静默通过）
normalize() { # $1=jq程序 $2=输入 $3=输出 $4=名称
  if ! jq -S "$1" "$2" >"$3" 2>"$TMPD/jqerr"; then
    echo "[invariants] $4 不是合法 JSON（视为越界）:" >&2
    cat "$TMPD/jqerr" >&2
    return 1
  fi
}

# 取 base 版 blob；文件在 base 不存在 ⇒ 视为新增文件，违规
base_blob() { # $1=工作区绝对路径
  local rel
  rel="$(realpath --relative-to="$REPO_ROOT" "$1")"
  "${GITR[@]}" show "$BASE:$rel" >"$TMPD/base.blob" 2>/dev/null
}

# 语义比对：归一化 base/工作区两版，非空 diff 即白名单外改动
check_semantic() { # $1=展示名 $2=工作区文件 $3=jq归一化程序
  if ! base_blob "$2"; then
    echo "[invariants] $1 在 $BASE 不存在（新增文件，禁止）"
    return 1
  fi
  normalize "$3" "$TMPD/base.blob" "$TMPD/norm.base" "$1@$BASE" || return 1
  normalize "$3" "$2" "$TMPD/norm.work" "$1@工作区" || return 1
  if cmp -s "$TMPD/norm.base" "$TMPD/norm.work"; then return 0; fi
  echo "[invariants] $1 出现白名单外改动（$BASE → 工作区），归一化后差异:"
  diff -u "$TMPD/norm.base" "$TMPD/norm.work" || true
  return 1
}

# 1) client.ts / schemas.ts 零差异（契约冻结；纯文本文件保留 git 法）
if ! "${GITR[@]}" diff --exit-code "$BASE" -- src/api/client.ts src/api/schemas.ts >"$TMPD/contract.diff" 2>&1; then
  echo "[invariants] 契约层出现改动（禁止）:"
  cat "$TMPD/contract.diff"
  fail=1
fi

# 2) tauri.conf.json 仅允许 app.windows[0].title —— 删该字段后整树比对，
#    行内合并进 title 行的任何兄弟字段都会在归一化结果中现形。
JQ_CONF='del(.app.windows[0].title)'
if ! check_semantic "tauri.conf.json" "$CONF_WORK" "$JQ_CONF"; then fail=1; fi

# 3) capabilities/default.json 仅允许 description 与 http:default 的 allow url 数组：
#    - del(.description)                 豁免描述文案；
#    - 仅 identifier=="http:default" 的条目开洞；
#    - 纯 {"url":…} 条目折叠为 {} 并 unique —— 增/删/改值/重排均豁免；
#    - 混入走私键的条目只删 .url 保留其余键 → 必现形；
#    - 其余一切（新增 permission、allow 改名、windows 变更等）全量参与比对。
JQ_CAP='def pure_url: (type == "object") and ((keys_unsorted - ["url"]) | length == 0);
del(.description)
| (.permissions // []) |= map(
    if (type == "object" and .identifier == "http:default")
    then .allow |= (((. // []) | map(if pure_url then {}
                                     elif type == "object" then del(.url)
                                     else . end)) | unique)
    else . end)'
if ! check_semantic "capabilities/default.json" "$CAP_WORK" "$JQ_CAP"; then fail=1; fi

# 4) Rust 侧零差异（保留 git 法）
rust_changed=$("${GITR[@]}" diff --name-only "$BASE" -- 'src-tauri/src/**' || true)
if [ -n "$rust_changed" ]; then
  echo "[invariants] Rust 源码出现改动（禁止）: $rust_changed"
  fail=1
fi

if [ "$fail" -eq 0 ]; then
  echo "[invariants] OK — 契约与 Tauri 白名单外区域零漂移（base=$BASE）"
fi
exit "$fail"
