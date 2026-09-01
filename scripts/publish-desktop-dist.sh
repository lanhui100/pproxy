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
#   token 来源：GitHub Secrets `VERCEL_TOKEN` 或 dev 主机 ~/pproxy/.pproxy.env 的 PPROXY_VERCEL_TOKEN
#
# 流程：按 latest.json 定位 exe+sig → 暂存（点号命名）→ 改写 latest.json 为 dl.ponyjob.top →
# 生成 vercel.json 缓存策略 → vercel link → vercel deploy --prod（纯静态，无函数）。
set -euo pipefail

BUNDLE_DIR="${1:?usage: $0 <bundle-dir> <latest.json>}"
LATEST_JSON="${2:?usage: $0 <bundle-dir> <latest.json>}"
: "${VERCEL_TOKEN:?VERCEL_TOKEN 未设置（GitHub Secrets: VERCEL_TOKEN 或 ~/pproxy/.pproxy.env: PPROXY_VERCEL_TOKEN）}"

[[ -f "$LATEST_JSON" ]] || { echo "latest.json 不存在: $LATEST_JSON"; exit 1; }

# 分发文件名 = latest.json 引用的名字（点号命名，与 GitHub 资产一致）
NAME=$(node -e "
const j = JSON.parse(require('fs').readFileSync(process.argv[1], 'utf8'));
const u = (j.platforms && j.platforms['windows-x86_64'] && j.platforms['windows-x86_64'].url) || '';
const rawName = u.split('/').pop() || '';
console.log(rawName.replace(/\s+/g, '.'));
" "$LATEST_JSON")
[[ "$NAME" == *_x64-setup.exe ]] || { echo "latest.json 未指向 _x64-setup.exe: '$NAME'"; exit 1; }

EXE=""
for cand in "$BUNDLE_DIR/$NAME" "$BUNDLE_DIR/${NAME/Pony.Proxy_/Pony Proxy_}"; do
  [[ -f "$cand" ]] && { EXE="$cand"; break; }
done
[[ -n "$EXE" ]] || { echo "bundle 目录找不到 $NAME（及其空格变体）: $BUNDLE_DIR"; exit 1; }
SIG="${EXE}.sig"
[[ -f "$SIG" ]] || { echo "缺少签名文件: $SIG"; exit 1; }

STAGE="$(mktemp -d)"
trap 'cd /; rm -rf "$STAGE" 2>/dev/null || true' EXIT
cp "$EXE" "$STAGE/$NAME"
cp "$SIG" "$STAGE/$NAME.sig"
cp "$LATEST_JSON" "$STAGE/latest.json"

# 改写 $STAGE/latest.json 内各平台下载地址为 https://dl.ponyjob.top/<filename>
node -e "
const fs = require('fs');
const p = process.argv[1];
const host = process.argv[2];
const d = JSON.parse(fs.readFileSync(p, 'utf8'));
if (d.platforms) {
  for (const plat of Object.values(d.platforms)) {
    if (plat.url) {
      const fn = plat.url.split('/').pop().replace(/\s+/g, '.');
      plat.url = 'https://' + host + '/' + fn;
    }
  }
}
fs.writeFileSync(p, JSON.stringify(d, null, 2));
" "$STAGE/latest.json" "dl.ponyjob.top"

# 生成 vercel.json 缓存策略：latest.json 及时校验；exe 与 sig 边缘永久缓存
cat > "$STAGE/vercel.json" << 'EOF'
{
  "headers": [
    {
      "source": "/latest.json",
      "headers": [
        {
          "key": "Cache-Control",
          "value": "public, max-age=0, must-revalidate"
        }
      ]
    },
    {
      "source": "/(.*\\.(?:exe|sig))",
      "headers": [
        {
          "key": "Cache-Control",
          "value": "public, max-age=31536000, immutable"
        }
      ]
    }
  ]
}
EOF

cd "$STAGE"
# 优先带 --scope pony7 link，若无团队权限则回退默认 scope
npx --yes vercel@latest link --yes --project pony-dsk --scope pony7 --token "$VERCEL_TOKEN" 2>/dev/null || \
  npx --yes vercel@latest link --yes --project pony-dsk --token "$VERCEL_TOKEN" >/dev/null

npx --yes vercel@latest deploy --prod --yes --token "$VERCEL_TOKEN"

echo "发布完成：https://dl.ponyjob.top/latest.json（$NAME）"
