#!/usr/bin/env bash
# publish-desktop-dist.sh — 将桌面端构建产物发布到 Vercel 静态分发（dl.ponyjob.top）。
#
# WHY：桌面端 updater 需要匿名、国内可达的静态分发源。私仓 GitHub 直链 404 且
# objects.githubusercontent.com 大陆不可达；旧方案（dev 主机 pproxy-server /dsk/，
# access.ponyjob.top）依赖 dev 在线，2026-08-30 曾因服务被停导致更新源 502 达 23h。
# 本脚本将分发迁到 Vercel（团队 pony7 / 项目 pony-dsk），GitHub Release 仍作归档源。
#
# 用法：
#   VERCEL_TOKEN=<token> scripts/publish-desktop-dist.sh <bundle-dir> <latest.json>
#     <bundle-dir>   tauri build 产物目录（含 *_x64-setup.exe 与 .sig）
#     <latest.json>  更新清单，platforms.*.url 必须已指向 https://dl.ponyjob.top/
#   token 来源：dev 主机 ~/pproxy/.pproxy.env 的 PPROXY_VERCEL_TOKEN
#
# 流程：暂存 exe + sig + latest.json → vercel link（幂等）→ vercel deploy --prod
# （纯静态目录，无函数）。latest.json 由 Vercel 以 must-revalidate 提供，
# updater 不会读到陈旧清单；每次发布整目录替换，目录内只保留当前版本。
set -euo pipefail

BUNDLE_DIR="${1:?usage: $0 <bundle-dir> <latest.json>}"
LATEST_JSON="${2:?usage: $0 <bundle-dir> <latest.json>}"
: "${VERCEL_TOKEN:?VERCEL_TOKEN 未设置（dev 主机 ~/pproxy/.pproxy.env: PPROXY_VERCEL_TOKEN）}"

[[ -f "$LATEST_JSON" ]] || { echo "latest.json 不存在: $LATEST_JSON"; exit 1; }
EXE=$(ls "$BUNDLE_DIR"/*_x64-setup.exe 2>/dev/null | head -1)
SIG=$(ls "$BUNDLE_DIR"/*_x64-setup.exe.sig 2>/dev/null | head -1)
[[ -n "$EXE" && -n "$SIG" ]] || { echo "bundle 目录缺少 *_x64-setup.exe / .sig: $BUNDLE_DIR"; exit 1; }

# 统一点号命名（tauri 产物名带空格；GitHub 资产与 latest.json URL 均用点号）
EXE_OUT="$(basename "$EXE" | tr ' ' '.')"
SIG_OUT="$(basename "$SIG" | tr ' ' '.')"
grep -q "$EXE_OUT" "$LATEST_JSON" || { echo "latest.json 未引用 $EXE_OUT（URL 必须与分发文件名一致）"; exit 1; }

STAGE="$(mktemp -d)"
trap 'rm -rf "$STAGE"' EXIT
cp "$EXE" "$STAGE/$EXE_OUT"
cp "$SIG" "$STAGE/$SIG_OUT"
cp "$LATEST_JSON" "$STAGE/latest.json"

cd "$STAGE"
npx --yes vercel@latest link --yes --project pony-dsk --token "$VERCEL_TOKEN" >/dev/null
npx --yes vercel@latest deploy --prod --yes --token "$VERCEL_TOKEN"

echo "发布完成：https://dl.ponyjob.top/latest.json"
