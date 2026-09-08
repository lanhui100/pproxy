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

# 凭据检查：支持 Cloudflare R2 / S3 兼容对象存储，或 Vercel 静态托管
if [[ -z "${R2_BUCKET:-${S3_BUCKET:-}}" && -z "${VERCEL_TOKEN:-}" ]]; then
  echo "错误：未配置分发凭据。请提供 R2_BUCKET (S3_BUCKET) 或 VERCEL_TOKEN"
  exit 1
fi

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

# 同步复制 CLI 全平台二进制，确保 pproxy update 能从分发网关直接下载
for bin in "$BUNDLE_DIR"/pproxy-*; do
  if [[ -f "$bin" ]]; then
    cp "$bin" "$STAGE/"
    echo "包含 CLI 二进制分发: $(basename "$bin")"
  fi
done

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

# 生成 vercel.json 缓存策略：latest.json 及时校验；exe、sig 与 pproxy 二进制边缘永久缓存
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
      "source": "/(.*\\.(?:exe|sig)|pproxy-.*)",
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

# ---- 途径 A: Cloudflare R2 / S3 兼容对象存储直传（推荐，零出网流量费，版本永久保留） ----
if [[ -n "${R2_BUCKET:-${S3_BUCKET:-}}" ]]; then
  BUCKET="${R2_BUCKET:-$S3_BUCKET}"
  ENDPOINT="${R2_ENDPOINT:-${S3_ENDPOINT:-}}"
  ENDPOINT_FLAG=""
  if [[ -n "$ENDPOINT" ]]; then
    ENDPOINT_FLAG="--endpoint-url $ENDPOINT"
  fi

  echo "[R2/S3] 同步分发资产到存储桶: $BUCKET"
  if command -v aws >/dev/null 2>&1; then
    # 1. 上传可执行文件与签名（immutable 永久缓存）
    aws s3 sync "$STAGE" "s3://$BUCKET/" $ENDPOINT_FLAG \
      --exclude "latest.json" --exclude "vercel.json" \
      --cache-control "public, max-age=31536000, immutable"
    # 2. 上传最新清单 latest.json（即时校验）
    aws s3 cp "$STAGE/latest.json" "s3://$BUCKET/latest.json" $ENDPOINT_FLAG \
      --cache-control "public, max-age=0, must-revalidate"
    echo "[R2/S3] 发布完成：https://dl.ponyjob.top/latest.json（$NAME）"
    exit 0
  else
    echo "WARN: 未找到 aws cli，回退尝试通过 Vercel 静态分发..."
  fi
fi

# ---- 途径 B: Vercel 静态分发（聚合最近历史版本，防止历史版本瞬间 404） ----
if [[ -n "${VERCEL_TOKEN:-}" ]]; then
  if command -v gh >/dev/null 2>&1; then
    echo "[Vercel] 补充最近历史版本安装包以防历史 404..."
    HIST_TAGS=$(gh release list --limit 6 --json tagName --jq '.[].tagName' 2>/dev/null | grep '^desktop-v' | grep -v "${TAG:-none}" | head -n 2 || true)
    for htag in $HIST_TAGS; do
      echo "[Vercel] 聚合历史资产: $htag"
      gh release download "$htag" --dir "$STAGE" --pattern "*_x64-setup.exe*" 2>/dev/null || true
    done
  fi

  cd "$STAGE"
  # 优先从环境变量 VERCEL_SCOPE / VERCEL_ORG_ID 读取，未指定时自适应，彻底解耦硬编码团队名
  SCOPE_ARGS=()
  if [[ -n "${VERCEL_SCOPE:-${VERCEL_ORG_ID:-}}" ]]; then
    SCOPE_ARGS=("--scope" "${VERCEL_SCOPE:-$VERCEL_ORG_ID}")
  fi
  npx --yes vercel@latest link --yes --project pony-dsk "${SCOPE_ARGS[@]}" --token "$VERCEL_TOKEN" 2>/dev/null || \
    npx --yes vercel@latest link --yes --project pony-dsk --token "$VERCEL_TOKEN" >/dev/null

  npx --yes vercel@latest deploy --prod --yes --token "$VERCEL_TOKEN"
  echo "发布完成：https://dl.ponyjob.top/latest.json（$NAME）"
  exit 0
fi

echo "错误：未完成任何有效分发发布"
exit 1
