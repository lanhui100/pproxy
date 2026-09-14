# 故障排查手册

## 症状速查

### 国内 DNS 解析失败（NXDOMAIN）
- **原因**：子域名含敏感词（如 "proxy"）被国内 DNS 过滤（ADR-003）
- **排查**：`dig @223.5.5.5 <域名>` vs `dig @8.8.8.8 <域名>`——海外通国内不通即命中
- **解决**：换中性子域名（edge/vedge/access）；勿用 /etc/hosts 硬编码（IP 变更脆弱）

### OpenAI 返回 403 unsupported_country_region_territory
- **原因**：请求走了 CF Worker 上游（AS13335 被 OpenAI 整段拉黑）
- **解决**：确认路由走 vercel 上游（config.json route_upstreams.openai=vercel）

### zen 返回 RegionError
- **原因**：同上（CF→CF 内部流量传播入口国家）
- **解决**：走 vercel 上游

### zen 返回 DataPolicyError
- **原因**：账号未 opt-in 数据政策（非链路问题）
- **解决**：访问错误信息中的 opencode.ai workspace 链接点同意

### 上游 403 Unauthorized
- **原因**：X-Proxy-Secret 不匹配
- **排查**：config.json worker_secret / upstreams.*.secret 与 Worker vars / Vercel env 是否一致

### Google 429
- **原因**：CF 出口 IP 被 Google 限流（瞬时）
- **解决**：重试；持续出现则考虑该路由切 vercel

### Facebook/上游偶发 000 或 Broken pipe
- **原因**：上游瞬时抖动或客户端提前断开（日志可见 client error: Broken pipe，无害）
- **解决**：重试；持续失败按"上游可达性"检查（DEPLOY.md 监控点）

### LLM 长响应被掐断（约 120s，实码声明；平台上限 300s）
- **原因**：本仓 edge 实码声明 maxDuration=120（deploy/vercel/vercel.json:6，api/proxy.js:1），300s 仅为 Vercel Hobby/Fluid 平台上限
- **缓解**：流式模式一般够用；超长任务等待 P2 自定义上游

### vercel.app 域名返回 Login 页面
- **原因**：项目 ssoProtection=all_except_custom_domains（vercel.app 域名有登录墙）
- **解决**：始终使用自定义域名 vedge.example.com

### 服务无响应
```bash
systemctl status pproxy            # 进程状态
journalctl -u pproxy -n 50         # 最近日志
ss -tlnp | grep 8899               # 端口监听
curl -s http://127.0.0.1:8899/     # 网关健康
```

### CF Worker 部署报 Authentication error (code 10000)
- **原因**：wrangler token 权限不足（无 zone DNS 写权限）
- **现状**：custom_domain 方式可部署成功；直接 DNS API 不可用
- **解决**：域名/DNS 变更走 CF 控制台手动操作

## 诊断工具

```bash
# 出口 IP 验证（经 Worker）——密钥从 .secrets.env 读取，禁止写入文档（2026-08 审计整改）
curl -s "https://edge.example.com/get?url=https://httpbin.org/ip" -H "X-Proxy-Secret: $(source .secrets.env && echo $PROXY_SECRET)"
# 期望: CF 出口 IP（104.22.x / 2a06:98c0::），不含真实家庭出口 IP（泄露检查）

# 出口 IP 验证（经 Vercel）
curl -s "https://vedge.example.com/api/proxy?url=https%3A%2F%2Fhttpbin.org%2Fip" -H "X-Proxy-Secret: $(source .secrets.env && echo $PROXY_SECRET)"
# 期望: 3.x.x.x（AWS us-east）
```

## 监控源状态异常（M3+）

| 现象 | 判定 | 处理 |
|------|------|------|
| `/api/quota` sources.vercel=error，日志 `vercel collect failed ... http status 400` | **已知问题**：Vercel `/v1/usage` 端点行为漂移（2026-08-22 实测各日期格式均 400 invalid_from_date；token 本身经 v9/projects 200 验证有效）。Hobby 计划本无可用用量数据 | 无需处理；Dashboard 徽标语义即 error。待 Vercel 端点明确或升级 Pro 后重测 |
| sources.cf=error | GraphQL errors 或网络失败，每 tick 自动重试 | 检查 `.pproxy.env` 凭据有效期（dashboard 可撤销）；journalctl 看 monitor warn 详情 |

## 隧道 401 / 双出口齐挂

> 范围：CF gate + Vercel gate 双 WS 出口（`tunnel.json` 双端点，
> `wss://gate.example.com/ws,wss://vgate.example.com/api/ws`）。
> 单腿 401 走 failover 是正常架构行为；本章只处理"业务不可用"的组合。
> 全章占位写法：真实 token / hash 全值 / 真实域名一律不入库，
> 下文 `<token-file>` / `<tunnel_token>` / `<hash>` / `abcd1234` 均为占位。

### 1. 症状速查表（以 kind 为准，禁止把 denied 误判为 401）

kind 口径见 `engine_tunnel.rs::classify_probe_error`：
`auth401`（Upgrade 401）/ `denied`（门禁 acl/colo/egress）/
`timeout` / `closed` / `other` / `no_token`。

| 双端 kind | 含义 | 先查 |
|-----------|------|------|
| 双 `auth401` | 手持 token 陈旧（轮换后未重录），或双端 hash 同旧 | §4 时间线核对 → 桌面端重录授权码 |
| 一 `ok` 一 `401` | 双端 `TUNNEL_TOKEN_HASH` 不同源（一端轮换漏配 / 配了未 deploy 生效） | 以 ok 端为准续命；按 §4 补齐落后端 |
| 混 `denied` | 门禁（acl / colo / egress 地理），**非鉴权问题**，不得按 401 轮换 token | 按 host 走合规出口（Google 系走 Vercel 优先，常规走 CF） |
| 混 `timeout` / `closed` / `other` | 网络 / Vercel Fluid 冷启动 / 半死隧道 | 重试；持续则查网络与部署状态 |

注意：OpenAI / Claude 类 host 的 CF 腿注定失败（平台拒绝约 2.4s 再
failover 到 Vercel，全程 5.5s 起步）——冒烟这类 host **只读 Vercel 腿**，
CF 腿的 `denied` / `timeout` 属预期，不判 401；单站连通性预算按 10s 看
（4s 预算对 failover 链路必然误报）。

### 2. 自检三值取证（分叉即先查本地 H2）

` tunnel_self_check`（逐 gate 自检）返回字段（见 `lib.rs::tunnel_self_check` /
`proxy_tunnel_get`，指纹均为 SHA-256 前 8 hex，只含 fp8 无爆破材料）：

- `fingerprint`：当前有效 token 指纹；
- `fp_fallback`：本地备份文件指纹；`fp_keyring`：系统凭据库指纹；
- `cred_winner`：`keyring` / `keyring(diverged)` / `fallback` / `none`
  （另有调试 `dev_file`）；`cred_meta`：`{last_write_ts, source, fp8}`
  上次写入审计；`gates[]`：逐端点 `{name, url, ok, kind, ms/error}`。

判定：

- `fp_fallback == fp_keyring == fingerprint`：本地单一来源，先查服务端 H1（§4）。
- `fp_fallback != fp_keyring`（`cred_winner=keyring(diverged)`）：**本地分叉 H2**，
  先查本地，排除前不得定 H1；内存以 keyring 为准并自修复 fallback（旧副本留 `.bak`）。
- `fingerprint=null` + `cred_error` 含"凭据损坏或编码不兼容"：09-01 编码事故（§5），
  经桌面端重贴授权码，禁外部直写。
- `cred_meta.fp8` 与 `fingerprint` 不一致：读后又被写过（内存-磁盘分裂），
  按 `cred_meta.source/last_write_ts` 追写入来源。

自检单针探针用的是本机 token，只定"本机状态"；最终裁决以 §3 双冒烟为准。

### 3. 双冒烟 oracle（为准，不以自检单针为准）

```bash
node scripts/collect-401-evidence.mjs --token-file <f> [--out evidence/]
# 端点可用 CF_GATE_WS / VERCEL_GATE_WS / CF_GATE_DEBUG / VERCEL_GATE_DEBUG 覆盖
```

产出：`local-fingerprint.json`（仅 fp8）、`debug-cf.json` / `debug-vercel.json`
（免鉴权 `{set,len}`）、`smoke-cf.txt` / `smoke-vercel.txt`
（同 token 双端强冒烟，记 OK/TLS / 401 / denied / timeout）、`answers.md`（取证三问模板）。

红线：包内只允许 fp8 / set / len / ok / err 分类 / url / 脱敏日志行；
禁明文 token、`pony-gate://` 负载、`TUNNEL_TOKEN_HASH` 全值——脚本末尾有违禁
pattern 门禁，命中即 `exit 3` 禁止发送。取证包 token-file 的 fp8 必须与桌面端
自检 fp8 一致，否则先查本地分叉（H2）。

### 4. 轮换时间线核对（口径见 DEPLOY.md 凭据卫生）

1. `printf '%s' '<tunnel_token>' | sha256sum` 取 hash——**禁用 `echo`**（防尾换行污染）；
2. CF：`printf '%s' '<hash>' | npx wrangler versions secret put TUNNEL_TOKEN_HASH`
   → `npx wrangler versions deploy <version-id>`（只 put 不 deploy 不生效），记录 version-id；
3. Vercel：更新 env 后**必须重新部署**（`workflow_dispatch` 全量部署或 `vercel --prod`），
   改 env 不自动对 Fluid 实例生效，记录 deployment id；
4. 双端 `/debug` 对 `{set,len}`：`set=false` 即缺失；**`len!=64` 即写脏**
  （带换行 / 截断 / 非 hex，sha256 hex 定长 64）；
5. 留痕：时间 / 操作人 / 双端 hash 前 8 / version-id / redeploy 顺序；
   用 §3 验证双端 OK 后再分发 `pony-gate://` 口令；
6. 桌面端重录授权码前，先对照自检 fingerprint（H2 本地分叉未排除前不得定 H1）。

### 5. 索引：09-01 外部工具 UTF-8 直写 keyring 编码事故

详见 `docs/ops/DESKTOP-TROUBLESHOOTING.md`「凭据编码污染事故：『隧道未配置』误报 +
全站 401 · 2026-09-01」：当日 token 轮换时新 token 被脚本以 **UTF-8 字节**
直接 `CredWrite`，而桌面端 keyring 3.6.3 一律按 **UTF-16** 解码 blob
（其 `set_password` 亦按 UTF-16 写入）→ `get_password()` 报
`Err("Data is not UTF-8 encoded")` 被静默吞成"未配置"；同步口令导入把旧 token
直送内存 watch（不读凭据），引擎持旧 token 拨 gate → 401；接口拨测每次现读凭据
故能升级 WS，造成"接口绿、站点红"。

纪律：写入 `tunnel_token.pony-desktop` 凭据**必须经桌面端**
（设置页 / 同步口令 / 连接口令）**或 keyring 兼容工具（UTF-16 blob），禁 UTF-8
字节直接 CredWrite**。已固化防御：保存后直发 secret 到 watch、凭据 Err 单独报
"凭据损坏"、`proxy_tunnel_get` 暴露 `cred_error` 与指纹、`tunnel_self_check`
一键逐 gate 自检。
