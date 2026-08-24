#!/usr/bin/env bash
# 不变量门禁（M7 SPEC §6.3）：契约层零漂移；src-tauri 仅白名单例外可动。
# 用法: bash scripts/check-invariants.sh <base-ref>   （如 HEAD、origin/master）
set -euo pipefail
BASE="${1:?usage: check-invariants.sh <base-ref>}"
cd "$(dirname "$0")/.."

fail=0

# 1) client.ts / schemas.ts 零差异（契约冻结）
if ! git diff --exit-code "$BASE" -- src/api/client.ts src/api/schemas.ts >/tmp/inv-contract.diff 2>&1; then
  echo "[invariants] 契约层出现改动（禁止）:"
  cat /tmp/inv-contract.diff
  fail=1
fi

# 2) tauri.conf.json 仅允许 title 单字段
conf_changed=$(git diff "$BASE" -- src-tauri/tauri.conf.json | grep -E '^[+-][^+-]' | grep -v '"title"' || true)
if [ -n "$conf_changed" ]; then
  echo "[invariants] tauri.conf.json 出现白名单外改动（仅允许 app.windows[0].title）:"
  printf '%s\n' "$conf_changed"
  fail=1
fi

# 3) capabilities/default.json 仅允许 url/description/结构符号变化
cap_changed=$(git diff "$BASE" -- src-tauri/capabilities/default.json | grep -E '^[+-][^+-]' | grep -vE '^\s*[+-]?\s*("url"|"description"|\{|\}|\[|\])' || true)
if [ -n "$cap_changed" ]; then
  echo "[invariants] capabilities 出现白名单外改动（仅允许 http scope url 与 description）:"
  printf '%s\n' "$cap_changed"
  fail=1
fi

# 4) Rust 侧零差异
rust_changed=$(git diff --name-only "$BASE" -- 'src-tauri/src/**' || true)
if [ -n "$rust_changed" ]; then
  echo "[invariants] Rust 源码出现改动（禁止）: $rust_changed"
  fail=1
fi

if [ "$fail" -eq 0 ]; then
  echo "[invariants] OK — 契约与 Tauri 白名单外区域零漂移（base=$BASE）"
fi
exit "$fail"
