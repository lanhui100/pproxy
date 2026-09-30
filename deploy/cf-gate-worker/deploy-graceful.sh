#!/usr/bin/env bash
# gate worker 优雅不下线部署（版本化灰度，零下线）
#
# 对应 SOP：docs/ops/DEPLOY.md「更新 CF Worker（gate / edge，优雅不下线 SOP）」
# 配套文档：docs/ops/DEPLOY.md §0 优雅不下线部署总则 / §更新 CF Worker
# 纪律：禁止裸 `wrangler deploy`；upload 新版本（线上不动）→ @5 灰度观察 →
# @50 → @100；异常随时 rollback。全程新旧版本共存切流，服务零下线。
#
# 用法（对抗审核修复：CLI 参数真实解析，不静默忽略）：
#   export CLOUDFLARE_API_TOKEN=<token>   # 或先 wrangler login
#   bash deploy/cf-gate-worker/deploy-graceful.sh \
#       [--observe-sec 120] [--tag <备注>] [--verify-url <URL>] [--verify-token <tunnel token>]
#
# 参数：
#   --observe-sec <s>   灰度观察秒数（默认 120）
#   --tag <备注>        版本备注（默认 deploy-graceful）
#   --verify-url <URL>  验证端点（默认 https://gate.example.com/debug，开源占位；
#                       生产必须显式传入真实域名——见 README 开源中立形态）
#   --verify-token <t> 真实 WS 隧道会话验证用 tunnel token（可选；提供时灰度窗口
#                       会额外做一次 wss 首帧握手检查，对齐 DEPLOY.md「仅 /debug 不够」口径）
#
# 退出码：0=全量部署完成；1=任一步失败（按需回滚，见输出指引）。
set -euo pipefail

WORKER_DIR="$(cd "$(dirname "$0")" && pwd)"
cd "$WORKER_DIR"

OBSERVE_SEC=120
MESSAGE="deploy-graceful"
VERIFY_URL="https://gate.example.com/debug"
VERIFY_TOKEN=""

while [ $# -gt 0 ]; do
  case "$1" in
    --observe-sec) OBSERVE_SEC="$2"; shift 2 ;;
    --tag) MESSAGE="$2"; shift 2 ;;
    --verify-url) VERIFY_URL="$2"; shift 2 ;;
    --verify-token) VERIFY_TOKEN="$2"; shift 2 ;;
    -h|--help)
      sed -n '2,20p' "$0"
      exit 0 ;;
    *)
      echo "ERROR: 未知参数 $1（支持 --observe-sec / --tag / --verify-url / --verify-token）" >&2
      exit 1 ;;
  esac
done

if [ "$VERIFY_URL" = "https://gate.example.com/debug" ]; then
  echo "WARN: --verify-url 未指定，使用开源占位域名（生产部署请显式传真实域）" >&2
fi

step() { echo "── [$1/6] $2"; }

# [1/6] 认证检查 + 离线打包校验
step 1 "认证检查 + 离线打包校验 (wrangler versions upload --dry-run)"
if [ -z "${CLOUDFLARE_API_TOKEN:-}" ]; then
  # 未设 token 时允许 wrangler login 会话（whoami 快速探测）
  if ! npx wrangler whoami >/dev/null 2>&1; then
    echo "ERROR: 未认证。请 export CLOUDFLARE_API_TOKEN=<token> 或先 wrangler login" >&2
    exit 1
  fi
fi
npx wrangler versions upload --dry-run >/dev/null

# [2/6] 上传新版本（不影响线上：旧版本继续 100% 服务）
step 2 "上传新版本（线上不动）"
UPLOAD_OUT="$(npx wrangler versions upload --message "$MESSAGE")"
echo "$UPLOAD_OUT" | grep -E "Version ID|version" || true
# 提取版本 ID：锚定 wrangler 4 的 "Version ID: <32-hex>" 输出行（对抗审核：正则收窄，
# 避免误抓输出中其他 32 位 hex）
VERSION_ID="$(echo "$UPLOAD_OUT" | grep -Eo 'Version ID: [0-9a-f]{32}' | awk '{print $3}' | head -1)"
if [ -z "${VERSION_ID:-}" ]; then
  echo "ERROR: 未能从 upload 输出解析 Version ID" >&2
  echo "完整输出：" >&2
  echo "$UPLOAD_OUT" >&2
  exit 1
fi
echo "  new version: $VERSION_ID"

# 观察窗口内的验证函数：/debug + （可选）真实 WS 首帧握手
verify_health() {
  local stage="$1"
  echo "  verify($stage): $VERIFY_URL"
  curl -sf -o /dev/null -w "    /debug HTTP %{http_code}\n" "$VERIFY_URL" || {
    echo "ERROR: $stage /debug 校验失败——执行回滚：" >&2
    echo "  npx wrangler versions list   # 查上一版本 ID" >&2
    exit 1
  }
  if [ -n "$VERIFY_TOKEN" ]; then
    echo "  verify($stage): wss 隧道首帧握手（真实 WS 会话，对齐 DEPLOY.md 口径）"
    WS_HOST="${VERIFY_URL#https://}"
    WS_HOST="${WS_HOST%%/*}"
    # 用 node 发起 wss 连接：Bearer 鉴权 → 首帧 {host,port} → 期待 {ok:true}
    node -e "
      const WebSocket = require('ws');
      const ws = new WebSocket('wss://${WS_HOST}/ws', { headers: { Authorization: 'Bearer ${VERIFY_TOKEN}' } });
      const t = setTimeout(() => { console.error('WS 握手超时'); process.exit(1); }, 10000);
      ws.on('open', () => ws.send(JSON.stringify({ host: 'cp.cloudflare.com', port: 443 })));
      ws.on('message', (d) => {
        clearTimeout(t);
        try {
          const v = JSON.parse(d.toString());
          if (v.ok === true) { console.log('    WS ok:true via=' + (v.via || 'direct')); process.exit(0); }
          console.error('WS denied: ' + (v.reason || '?')); process.exit(1);
        } catch { console.error('WS 首帧非 JSON'); process.exit(1); }
      });
      ws.on('error', (e) => { clearTimeout(t); console.error('WS error: ' + e.message); process.exit(1); });
    " || {
      echo "ERROR: $stage WS 隧道会话校验失败——执行回滚：" >&2
      echo "  npx wrangler versions list   # 查上一版本 ID" >&2
      exit 1
    }
  fi
}

# [3/6] 灰度 5% + 观察窗口
step 3 "灰度 5%（${OBSERVE_SEC}s 观察）"
npx wrangler versions deploy "$VERSION_ID@5" --message "$MESSAGE@5%"
echo "  observe ${OBSERVE_SEC}s ..."
sleep "$OBSERVE_SEC"
verify_health "灰度5%"

# [4/6] 加量 50% + 观察
step 4 "加量 50%（${OBSERVE_SEC}s 观察）"
npx wrangler versions deploy "$VERSION_ID@50" --message "$MESSAGE@50%"
sleep "$OBSERVE_SEC"
verify_health "灰度50%"

# [5/6] 全量
step 5 "全量 100%"
npx wrangler versions deploy "$VERSION_ID@100" --message "$MESSAGE@100%"

# [6/6] 终验
step 6 "终验"
verify_health "终验"
echo "✅ 部署完成（版本 $VERSION_ID）。异常回滚：npx wrangler rollback <上一可用版本ID>"
