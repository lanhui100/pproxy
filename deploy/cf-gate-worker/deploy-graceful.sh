#!/usr/bin/env bash
# gate worker 优雅不下线部署（版本化灰度，零下线）
#
# 对应 SOP：docs/ops/DEPLOY.md「更新 CF Worker（gate / edge，优雅不下线 SOP）」
# 配套文档：docs/ops/DEPLOY.md §0 优雅不下线部署总则 / §更新 CF Worker
# 纪律：禁止裸 `wrangler deploy`；upload 新版本（线上不动）→ @5 灰度观察 →
# @50 → @100；异常随时 rollback。全程新旧版本共存切流，服务零下线。
#
# 用法：
#   export CLOUDFLARE_API_TOKEN=<token>   # 或先 wrangler login
#   bash deploy/cf-gate-worker/deploy-graceful.sh [--observe-sec 120] [--tag <备注>]
#
# 退出码：0=全量部署完成；1=任一步失败（按需回滚，见输出指引）。
set -euo pipefail

WORKER_DIR="$(cd "$(dirname "$0")" && pwd)"
OBSERVE_SEC="${OBSERVE_SEC:-120}"
MESSAGE="${MESSAGE:-deploy-graceful}"
VERIFY_URL="${VERIFY_URL:-https://gate.ponygo.fun/debug}"

cd "$WORKER_DIR"

step() { echo "── [$1/6] $2"; }

# [1/6] 认证与离线打包校验
step 1 "认证检查 + 离线打包校验 (wrangler versions upload --dry-run)"
npx wrangler versions upload --dry-run >/dev/null

# [2/6] 上传新版本（不影响线上：旧版本继续 100% 服务）
step 2 "上传新版本（线上不动）"
UPLOAD_OUT="$(npx wrangler versions upload --message "$MESSAGE")"
echo "$UPLOAD_OUT" | grep -E "Version ID|version" || true
# 提取 32 位十六进制版本 ID（wrangler 4 输出形如 "Version ID: abc...def"）
VERSION_ID="$(echo "$UPLOAD_OUT" | grep -oE '[0-9a-f]{32}' | head -1)"
if [ -z "${VERSION_ID:-}" ]; then
  echo "ERROR: 未能从 upload 输出解析 Version ID" >&2
  exit 1
fi
echo "  new version: $VERSION_ID"

# [3/6] 灰度 5% + 观察窗口
step 3 "灰度 5%（$OBSERVE_SEC 秒观察）"
npx wrangler versions deploy "$VERSION_ID@5" --message "$MESSAGE@5%"
echo "  observe $OBSERVE_SEC s ..."
sleep "$OBSERVE_SEC"
echo "  verify: $VERIFY_URL"
curl -sf -o /dev/null -w "  /debug HTTP %{http_code}\n" "$VERIFY_URL" || {
  echo "ERROR: 灰度窗口 /debug 校验失败——执行回滚：" >&2
  echo "  npx wrangler versions list   # 查上一版本 ID" >&2
  exit 1
}

# [4/6] 加量 50% + 观察
step 4 "加量 50%（$OBSERVE_SEC 秒观察）"
npx wrangler versions deploy "$VERSION_ID@50" --message "$MESSAGE@50%"
sleep "$OBSERVE_SEC"
curl -sf -o /dev/null -w "  /debug HTTP %{http_code}\n" "$VERIFY_URL" || {
  echo "ERROR: 50% 窗口校验失败——回滚：" >&2
  exit 1
}

# [5/6] 全量
step 5 "全量 100%"
npx wrangler versions deploy "$VERSION_ID@100" --message "$MESSAGE@100%"

# [6/6] 终验
step 6 "终验"
curl -sf -o /dev/null -w "  /debug HTTP %{http_code}\n" "$VERIFY_URL"
echo "✅ 部署完成（版本 $VERSION_ID）。异常回滚：npx wrangler rollback <上一可用版本ID>"
