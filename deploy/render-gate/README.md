# render-gate — pproxy 第三出口（Render 美区隧道桥）

WS↔TCP 隧道桥，协议与 `deploy/cf-gate-worker/worker.js` 完全对齐，部署在 Render 免费容器（Oregon），
为 CONNECT 隧道提供出口地区固定（美国）的兜底，根治 CF HKG colo 下 Google
`FAILED_PRECONDITION: User location is not supported` 错误。

spec：`docs/product/specs/render-gate-egress/README.md`

## Render 建站步骤（用户操作，约 5 分钟）

1. 本目录代码需先 push 到 GitHub（Render 从 Git 拉取）。
2. [Render Dashboard](https://dashboard.render.com) → **New + → Web Service** → 连接 GitHub 仓库。
3. 配置：
   | 项 | 值 |
   |----|----|
   | Root Directory | `deploy/render-gate` |
   | Runtime | Node |
   | Build Command | `npm install` |
   | Start Command | `node server.js` |
   | Region | **Oregon (US West)** ← 关键，勿选 Singapore/Frankfurt |
   | Instance Type | **Free** |
   | Health Check Path | `/healthz` |
4. Environment Variables：
   | Key | Value |
   |-----|-------|
   | `TUNNEL_TOKEN_HASH` | 隧道 token 的 SHA-256 hex（**与 CF gate 的 `TUNNEL_TOKEN_HASH` 完全一致**；本地计算：`python -c "import hashlib;print(hashlib.sha256(b'<token>').hexdigest())"`） |
   | `KEEPALIVE_URL` | 部署完成后可见域名，回填 `https://<app>.onrender.com`（防免费层 15min 休眠） |
5. 部署后验证：
   ```bash
   curl https://<app>.onrender.com/healthz   # {"ok":true}
   curl https://<app>.onrender.com/debug     # {"set":true}（hash 已配置）
   ```

## 接入 pproxy（桌面端）

隧道 URL 配置追加第二端点（桌面端 `engine_tunnel.rs` 原生支持逗号分隔多 gate 故障转移，denied 亦 fallover）：

```
wss://gate.ponyjob.top/ws,wss://<app>.onrender.com/ws
```

CF worker 已带 colo 门禁：HKG/MFM colo + Google 系 host → 秒拒 `unsupported_colo` → 自动落 Render。

## 运营注意（spec §5/§7）

- **Render 账号独占**：免费 750 实例小时/月为账号级共享，本服务常驻需 744h/月；同账号勿再跑第二个免费 Web Service。
- **保活三层**：`KEEPALIVE_URL` 自拨（10min）+ 建议叠加 UptimeRobot/cron-job.org 免费监控（5min ping `/healthz`）+ 桌面端 Dashboard 隧道拨测。
- **归账口径（已知偏差）**：桌面端 Dashboard 把 Render 流量计入 CF 桶（`classify_egress` 后续演化）；排障时注意"CF 桶流量 ≠ CF 出口"。
- **一级回滚**：从隧道 URL 配置中摘除 `,wss://<app>.onrender.com/ws` 即整体旁路本服务。
- **测试注入 env**（`RENDER_GATE_EXTRA_PORT` / `RENDER_GATE_ALLOW_PRIVATE`）仅 selftest 使用，生产绝不设置。

## 本地开发

```bash
npm install        # 生成/校验 package-lock.json
npm run selftest   # 23 项自检（鉴权/ACL/IP 变体/透传/Ping-Pong/并发/锁定）
```
