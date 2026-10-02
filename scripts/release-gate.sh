#!/usr/bin/env bash
# release-gate.sh — 桌面版发版校验门禁（preflight + 可选 post-release feed 校验）。
#
# 背景（2026-10-02 两类事故）：
#   1) GitHub "latest" 更新端点被 CLI 发布（v0.3.57，无 latest.json 资产）抢占 → 桌面端检查更新报
#      error sending request for url (https://access.ponygo.fun/dsk/latest.json)；
#   2) Dashboard 图例/图表关键 UI 曾有"发布后用户看不到"的隐患 → 以产物字符串门禁兜底。
# 本脚本把版本一致性、类型/单测/构建、产物 UI 字符串、更新 feed 双通道全部机械化为非零退出检查，
# 接入 desktop-release.yml（发版前/发版后）与本地人工发版前自检，防同类错误随发布复发。
#
# 用法：
#   scripts/release-gate.sh                          # 发版前自检（版本一致 + 前端 + 产物 UI）
#   scripts/release-gate.sh 0.3.63                   # 显式期望版本
#   scripts/release-gate.sh --feed                   # 额外校验线上更新 feed 双通道版本 == 期望版本（发版后跑）
#   scripts/release-gate.sh 0.3.63 --feed
set -euo pipefail

ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
cd "$ROOT"

FEED=0
declare -a POS
for a in "$@"; do
  [[ "$a" == "--feed" ]] && FEED=1 || POS+=("$a")
done
EXPECTED="${POS[0]:-$(python3 -c "import json; print(json.load(open('desktop/package.json'))['version'])")}"
# 容忍 tag 派生版本（desktop-v0.3.63 → "v0.3.63"）带来的前导 v
EXPECTED="${EXPECTED#v}"

fail() { echo "❌ GATE FAIL: $*" >&2; exit 1; }
pass() { echo "✅ $*"; }

# ---- 1) 版本一致性（三处必须一致：发版时漏 bump 会被立即拦下） ----
PKG_V="$(python3 -c "import json; print(json.load(open('desktop/package.json'))['version'])")"
TAURI_V="$(python3 -c "import json; print(json.load(open('desktop/src-tauri/tauri.conf.json'))['version'])")"
CARGO_V="$(grep -m1 '^version *=' desktop/src-tauri/Cargo.toml | cut -d'"' -f2)"
echo "versions: package.json=$PKG_V tauri.conf.json=$TAURI_V Cargo.toml=$CARGO_V (期望 $EXPECTED)"
[[ "$PKG_V" == "$TAURI_V" && "$PKG_V" == "$CARGO_V" ]] || fail "三处版本号不一致"
[[ "$PKG_V" == "$EXPECTED" ]] || fail "期望版本 $EXPECTED 与 package.json $PKG_V 不一致"
pass "版本一致性"

# ---- 2) 前端：类型检查 + 单测 + 生产构建 ----
echo "→ pnpm check (vue-tsc)"
(cd desktop && pnpm check) || fail "vue-tsc 类型检查未通过"
echo "→ pnpm test (vitest)"
(cd desktop && pnpm test) || fail "前端单测未通过"
echo "→ pnpm build (vite 生产构建)"
(cd desktop && pnpm build) || fail "前端生产构建失败"
pass "前端 check/test/build"

# ---- 3) 产物 UI 字符串门禁：关键 UI 文本缺失于构建产物 = 组件可能没渲染，直接拦 ----
# 说明：esbuild/vite 对 CJK 字面量按 utf-8 保留在产物中（本仓库已实证）；若未来 minifier 转义，grep 亦需同步适配。
MARKERS=("近 7 日用量" "近 24 小时用量" "实时网速" "今日请求")
for m in "${MARKERS[@]}"; do
  hits="$(grep -rl "$m" desktop/dist/assets/ 2>/dev/null | wc -l)"
  (( hits > 0 )) || fail "关键 UI 文本缺失于构建产物: 「$m」（图表图例可能未渲染/被误删）"
done
pass "产物 UI 字符串门禁（${#MARKERS[@]} 项）"

# ---- 4) （可选，发版后）更新 feed 双通道：GitHub latest + access.ponygo.fun 必须返回期望版本 ----
if (( FEED )); then
  echo "→ check-update-feed.sh $EXPECTED"
  bash scripts/check-update-feed.sh "$EXPECTED" || fail "更新 feed 未对齐期望版本 $EXPECTED（GitHub latest 可能被 CLI 发布抢占，参见 .agents/notes/implemented/bug-fix/2026-10-02-github-latest-feed-shadowed-by-cli-release.md）"
  pass "更新 feed 双通道"
fi

pass "release-gate 全部通过 → 版本 $EXPECTED 可发版"