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
#     <latest.json>  更新清单；platforms.windows-x86_64.url 的文件名段决定分发哪个
#                    产物（单一事实来源）。tauri 产物名带空格（Pony Proxy_X.Y.Z_…），
#                    分发名统一点号（Pony.Proxy_X.Y.Z_…），脚本自动按两种名字定位。
#   token 来源：dev 主机 ~/pproxy/.pproxy.env 的 PPROXY_VERCEL_TOKEN
#
# 流程：按 latest.json 定位 exe+sig → 暂存（点号命名）→ vercel link（幂等）→
# vercel deploy --prod（纯静态目录，无函数）。latest.json 以 must-revalidate 提供，
# updater 不会读到陈旧清单；每次发布整目录替换，目录内只保留当前版本。
set -euo pipefail

BUNDLE_DIR="${1:?usage: $0 <bundle-dir> <latest.json>}"
LATEST_JSON="${2:?usage: $0 <bundle-dir> <latest.json>}"
: "${VERCEL_TOKEN:?VERCEL_TOKEN 未设置（dev 主机 ~/pproxy/.pproxy.env: PPROXY_VERCEL_TOKEN）}"

[[ -f "$LATEST_JSON" ]] || { echo "latest.json 不存在: $LATEST_JSON"; exit 1; }

# 分发文件名 = latest.json 引用的名字（点号命名，与 GitHub 资产一致）
NAME=$(node -e "const j=JSON.parse(require('fs').readFileSync(process.argv[1],'utf8'));const u=j.platforms['windows-x86_64']&&j.platforms['windows-x86_64'].url||'';console.log(u.split('/').pop())" "$LATEST_JSON")
[[ "$NAME" == *_x64-setup.exe ]] || { echo "latest.json 未指向 _x64-setup.exe: '$NAME'"; exit 1; }

EXE=""
for cand in "$BUNDLE_DIR/$NAME" "$BUNDLE_DIR/${NAME/Pony.Proxy_/Pony Proxy_}"; do
  [[ -f "$cand" ]] && { EXE="$cand"; break; }
done
[[ -n "$EXE" ]] || { echo "bundle 目录找不到 $NAME（及其空格变体）: $BUNDLE_DIR"; exit 1; }
SIG="${EXE}.sig"
[[ -f "$SIG" ]] || { echo "缺少签名文件: $SIG"; exit 1; }

STAGE="$(mktemp -d)"
trap 'cd /; rm -rf "$STAGE"' EXIT
cp "$EXE" "$STAGE/$NAME"
cp "$SIG" "$STAGE/$NAME.sig"
cp "$LATEST_JSON" "$STAGE/latest.json"

cd "$STAGE"
npx --yes vercel@latest link --yes --project pony-dsk --token "$VERCEL_TOKEN" >/dev/null
npx --yes vercel@latest deploy --prod --yes --token "$VERCEL_TOKEN"

echo "发布完成：https://dl.ponyjob.top/latest.json（$NAME）"
