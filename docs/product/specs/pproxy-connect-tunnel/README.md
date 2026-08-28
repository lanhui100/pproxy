# PProxy 服务端网关 CONNECT 隧道 — Spec v0.2

> 目标：dev 等 Linux 服务器上的数据面网关支持 CONNECT 隧道（经 gate worker 中继 TLS 密文），
> 使封闭二进制（如 Antigravity CLI `agy`）不改端点即可完成 Google OAuth 等全流程。
> 完成度定义为「spec 对抗审核 → 分阶段实施 → 每阶段代码双审 + 测试门禁 → dev 实测 → 收口归档」。
>
> **v0.2 修订**：按 3 路独立对抗审核（architect=有条件通过 / security=有条件通过 / 红队证伪=不通过）完成修订，
> 采纳记录见 §8.1。核心变化：分流机制改为 hyper 原生 CONNECT upgrade（替换自读头方案）、
> 删除 direct 模式、基线声明修正（desktop 源在 HEAD 不编译）、env 双消费者耦合如实记录、回滚方案重写。

---

## 0. 状态与断点（每阶段完成后必须更新本节）

**状态**：`阶段 2 — doctor 隧道探针`（In Progress）
**当前断点**：阶段 1 全部单测通过（25/25，a3805b5）；准备开始 crates/cli doctor 追加 CONNECT 探针段。
**Next Action**：T2 —— 在 `crates/cli/src/cmd/doctor.rs` 追加零机密隧道探针段（裸 CONNECT 等 200，无需 token/env），并补单测。
**Resume Hint**：新会话恢复时，读本节 + §4 阶段表；按"最后一个非 Done 阶段"继续；实现细节以 §3.7 为准。
**阶段 3 前置核查（已完成，2026-08-28）**：dev `~/pproxy/.pproxy.env` 已含 `PPROXY_TUNNEL_GATE_URL` 与 `PPROXY_TUNNEL_TOKEN`（值未打印，systemd 已加载）→ T3 无需新增 env；`/etc/systemd/system/pproxy.service.d/override.conf` 存在（tailnet 管理地址，回滚时必须保留）；数据面绑定 `127.0.0.1:8899`，服务 active。

| 阶段 | 内容 | 状态 | 产出 / 证据 |
|------|------|------|------------|
| 阶段 0 | spec 编写 + 3 路对抗审核 + 修订 | **Done** | 本文档 v0.2 + §8.1 采纳记录 |
| 阶段 1 | crates/server 移植 CONNECT/WS 隧道 + 单测 + 双代码审核 | **Done** | `crates/server/src/connect.rs`、`gateway.rs`、`main.rs`、`Cargo.toml`；cargo test 25/25（a3805b5） |
| 阶段 2 | `pproxy doctor` 增加 CONNECT 隧道探针 | In Progress | `crates/cli/src/cmd/doctor.rs` |
| 阶段 3 | dev 部署（cargo build + restart）+ 实测 agy 登录链路 | Pending | curl 实测输出、`agy login` 结果 |
| 阶段 4 | 收口：DELIVERY.md、提交、push、本地同步 | Pending | commit hash |

## 1. 背景与目标

### 1.1 问题现场

dev 服务器（`~/pproxy`）已 `pproxy on`（`https_proxy=http://127.0.0.1:8899`），Antigravity CLI（`agy`，Go 封闭二进制）登录时 token 交换失败：

```
Got an error: token exchange failed: Post "https://oauth2.googleapis.com/token": Forbidden
```

### 1.2 实测证据链（2026-08-28，全部逐条验证）

| 环节 | 结果 |
|------|------|
| 登录 shell 代理注入 | `/etc/profile.d/pproxy.sh` → `https_proxy=http://127.0.0.1:8899` ✅ |
| 网关对 CONNECT 的处理 | 硬编码 403 直接写回并关闭（`crates/server/src/gateway.rs:322`，P0-1 注释"无 relay 路径"）✅ |
| 实测走网关访问 Google | `curl -x http://127.0.0.1:8899 https://oauth2.googleapis.com/token` → `CONNECT tunnel failed, response 403` ✅ |
| dev 主机直连 Google | `oauth2/www/aicode.googleapis.com`、`accounts.google.com` 全部 timeout（网络被墙）✅ |
| 现有路径中继路由 | 仅 anthropic / github / openai / xai，无 google ✅ |
| gate worker 出口连通性 | 临时加 google-oauth2 路由实测：worker 能到达 Google（真实 404），已删除 ✅ |

根因：`agy` 是封闭二进制，只会遵循 `HTTPS_PROXY` 发 `CONNECT oauth2.googleapis.com:443`，无法改写为 pproxy 的路径中继格式；而服务端网关不支持 CONNECT。

### 1.3 目标

1. 服务端数据面网关（`crates/server`，dev 上监听 `127.0.0.1:8899`）支持 CONNECT：白名单域经既有 gate worker `/ws` 隧道中继，TLS 端到端（网关与 worker 只见密文）。
2. 安全语义不变且收紧：默认拒绝（白名单外 CONNECT 仍 403），worker 侧鉴权/端口/私网封禁原样复用；新增数据面非回环绑定告警（S-P2-数据面，见 §3.5）。
3. `agy login` 在 dev 上全流程可用；`pproxy doctor` 可零机密一键自检该链路。

### 1.4 非目标

- **不改 worker**（`deploy/cf-gate-worker/worker.js` 零改动）。
- **不改桌面端**（`desktop/src-tauri` 零改动。注：该 crate 在 HEAD 存在既有编译破损，见 §2 资产表注记，属上游债，本 spec 不修复）。
- 不做本地 MITM / CA 信任链 / TLS 终结。
- **不做 direct 直连回退模式**（v0.1 的 `PPROXY_CONNECT_MODE` 已删除，见 §8.1 采纳 #3：过度设计 + 非回环绑定下构成无鉴权开放中继含云 metadata SSRF 面，且 dev 无价值场景）。
- 不做 per-token 隧道用量计量（CONNECT 无 token 概念，见 §3.5）。
- 不放行非 443 端口（server 端预检 403 + worker 端 `ALLOWED_PORTS` 保持 443，双重闸）。
- 不拦截 absolute-form（`GET http://host/path`）——继续走既有 axum 路径中继鉴权路径，行为零变化（§8.1 采纳 #1：拦截会回归 `http_proxy` 注入下的路径中继用法）。

## 2. 现状与可复用资产（代码事实）

| 资产 | 位置 | 与本方案的关系 |
|------|------|--------------|
| CONNECT→403 分支 | `crates/server/src/gateway.rs:311-334` `handle_conn` | 本方案改造点（peek 分支整体删除，改 hyper 层拦截） |
| hyper 服务适配层 | `gateway.rs:352-366` `RouterHyperAdapter`（hyper Service → axum Router） | 扩展为携带 `GatewayState` 并拦截 `method == CONNECT` |
| CONNECT/absolute-form 解析、直连回退 | `desktop/src-tauri/src/proxy/engine.rs` | 仅参考语义；**代码不逐字移植**（见下注） |
| WS 隧道客户端逻辑 | `desktop/src-tauri/src/proxy/engine_tunnel.rs`（Bearer token、首帧 `{host,port}`、等 `{"ok":true}`、写 200 前重试、Ping/Pong 保活） | **行为一致移植 + 配置访问重写**。⚠️ 该文件在 HEAD 与 `engine.rs:21-27` 的 `EngineConfig` 字段失配（引用已不存在的 `cfg.tunnel`，v0.3.13(cb44cf3) 后重构回归），desktop crate 当前编译不过——"E2E 已验证"仅对 cb44cf3 时点成立，移植时 establish 的配置读取按 §3.3 的 `TunnelConfig` 重写 |
| gate worker 隧道桥 | `deploy/cf-gate-worker/worker.js` `/ws`（TUNNEL_TOKEN_HASH 鉴权、仅 443、best-effort 私网封禁、`cloudflare:sockets` 中继） | 零改动，直接复用 |
| 白名单后缀匹配 | `desktop/src-tauri/src/proxy/whitelist.rs`（`normalize_host` + `suffix_match` + 区域别名表） | 移植 normalize+suffix **不含 alias**（§3.3；注意桌面 `alias_match` 含通用 `.com.xx` 规则，不可随 `matches()` 整体带入） |
| 隧道 env 双消费者（既有） | `crates/server/src/tunnel.rs`（`TunnelProvision::from_env` 读 `PPROXY_TUNNEL_GATE_URL`/`PPROXY_TUNNEL_TOKEN`，接受 ws:// 或 wss://）+ `api.rs:704-710`（`GET /api/tunnel/config` 明文下发，admin Bearer + tailnet 边界，桌面自动配置唯一来源） | 同名 env 亦作本方案数据面配置源；耦合与安全边界见 §3.4/§3.5 |
| 隧道 token 部署链 | `crates/cli/src/cmd/deploy.rs:393-453`（读取既有 token 并设置 worker `TUNNEL_TOKEN_HASH`，打印 `PPROXY_TUNNEL_GATE_URL`/`PPROXY_TUNNEL_TOKEN`；不生成 token） | 复用变量命名；token 溯源见 §5 |
| 依赖 | `crates/server/Cargo.toml` 已有 workspace `futures`（SinkExt/StreamExt/Split* 可直接用）；**仅缺 `tokio-tungstenite 0.24 (rustls-tls-webpki-roots)`**（与 axum 0.7/hyper 1 同基于 http 1.x，无版本冲突） | T1 新增 |
| doctor | `crates/cli/src/cmd/doctor.rs`（4 段探测：health/routes/route tests/data plane） | 阶段 2 追加零机密隧道探针段（§3.7） |
| 服务形态 | `systemd/pproxy.service`：`EnvironmentFile=/home/USER/pproxy/.pproxy.env`（User=dm）、`ExecStart=/home/USER/pproxy/target/release/pproxy-server`；dev 另有 drop-in `override.conf`（PPROXY_LISTEN_ADMIN） | T3 部署 = dev 上 `cargo build --release` + restart；回滚见 §5 |

## 3. 方案设计

### 3.1 总体数据流

```
agy (Go, 遵循 HTTPS_PROXY)
  │ CONNECT oauth2.googleapis.com:443
  ▼
crates/server 网关 127.0.0.1:8899（hyper http1, header_read_timeout=30s 既有）
  │ RouterHyperAdapter 拦截 method==CONNECT（authority-form uri 即 host:port）
  │ 判定：隧道已配置？port==443？host 命中 allowlist？
  ├─ 全过 → 先建立 WS 隧道（wss://GATE_URL, Bearer token, 首帧 {host,port},
  │          等 {"ok":true}，网络类失败重试 1 次；denied 不重试）
  │        成功后回 "200 Connection Established"
  │        → hyper::upgrade::on(req) 取升级流 → 双向透传
  │        （TLS 端到端：agy ↔ Google；worker 只见密文）
  └─ 任一不过/失败 → 403/502 + x-pproxy-reason（§3.6 枚举），无 200 不透传
```

**方案路线裁决（D6）**：采用 hyper 原生 CONNECT upgrade，而非自读头+回灌适配器。
理由：hyper 承担请求头解析、16KB 上限、400、30s header_read_timeout（慢速客户端占 permit 问题一并消解）、升级流字节衔接（客户端 200 前抢跑的 TLS ClientHello 不丢失）；`handle_conn` 的 peek/403 分支整体删除；absolute-form 与普通 HTTP 路径零改动。双 reviewer 独立提出该方案（§8.1 #1）。

### 3.2 改造点 A：`gateway.rs`

1. `handle_conn`：删除 peek/CONNECT/403 分支（gateway.rs:316-334），所有连接直接进 `serve_connection`。
2. `RouterHyperAdapter`：增加 `state: GatewayState` 字段（Clone 廉价，Arc 组成）；`call` 中若 `req.method() == Method::CONNECT` → 走 `connect::handle_connect(state, req)`（返回 axum Response），否则照旧委托 axum Router。
3. `serve_data_plane` 签名增加 state 参数（`main.rs` 调用点同步）；Semaphore 语义不变：CONNECT 隧道任务持 permit 至连接结束（§3.5 用量说明）。
4. `GatewayState` 增加 `tunnel: Option<Arc<TunnelConfig>>`；同步更新既有测试构造点（gateway.rs 测试 ×2、main.rs）。

### 3.3 改造点 B：新模块 `crates/server/src/connect.rs`

- `TunnelConfig { gate_url: String, token: String, allowlist: Vec<String> }`：
  - `from_env()`：`PPROXY_TUNNEL_GATE_URL` + `PPROXY_TUNNEL_TOKEN` + `PPROXY_TUNNEL_ALLOWLIST`（逗号分隔，trim，空段丢弃）；**任一缺失/为空 → None（fail-closed）**；两者只配其一同样 None + warn。
  - **`gate_url` 仅接受 `wss://`**（数据面强制；与 `tunnel.rs:24` 管理面宽松校验的偏离在注释中记录——管理面下发后由桌面端自校验，数据面 Bearer token 直上该 URL，明文 ws:// 不可接受）。
  - allowlist 条目启动时健壮性 warn：含 scheme/端口/前导点/空白/单标签过宽（如 `com`）→ 提示可疑。
- `allowlist_match(host, entries)`：移植 `normalize_host` + `suffix_match`（host==entry 或 `*.entry`，dot-boundary 防 `notgoogleapis.com`）。**明确不含**桌面 `alias_match`（含通用 `.com.xx` 区域规则）；区分性测试：`matches("googleapis.com.hk", ["googleapis.com"]) == false`（§8.1 采纳 #17）。
- `establish`（移植自 desktop engine_tunnel，配置访问重写为 `TunnelConfig`）：`tokio_tungstenite::connect_async` + `Authorization: Bearer <token>` + Text 首帧 `{"host","port"}` + 等 `{"ok":true}`，10s 首帧超时，应答 Ping/Pong。
  - **RETRY 语义（修订）**：worker 明确拒绝（`{"ok":false}`，即 ACL/token 问题）→ 不重试立即失败；仅网络类错误（建连超时/断开）重试 1 次（间隔 400ms）。重试仅在尚未向客户端写 200 时进行（本方案下 hyper 未回响应即未写 200，天然成立）。
- `relay`：双向透传（8KB 缓冲，select 驱动），WS Ping→Pong，Close→断开。
- 日志约定：CONNECT 目标 host 允许记入日志（worker 侧本就记录，排障必需）；**绝不记 token/Authorization**（S-P2-10 精神在此明确化）。

### 3.4 配置接线（env 双消费者耦合，如实记录）

`PPROXY_TUNNEL_GATE_URL` / `PPROXY_TUNNEL_TOKEN` 同时是：
- **管理面既有消费者**：`tunnel.rs::TunnelProvision::from_env` → `/api/tunnel/config`（桌面端自动配置唯一来源，admin Bearer + tailnet 边界内明文下发）。
- **本方案新增数据面消费者**：`connect.rs::TunnelConfig::from_env` → CONNECT 隧道。

在 dev 上设置这组 env（已设置，见 §0 预核查）即同时激活两者——此耦合为既成事实，非本 spec 引入；spec 义务是记录并保证两者校验独立（数据面额外强制 wss:// 与 allowlist 非空才启用）。
载体：`~/pproxy/.pproxy.env`（User=dm 属主、600、systemd `EnvironmentFile` 已挂载）；**机密只进该文件，systemd drop-in 只放非机密变量**（防 644 drop-in 泄 token）。config.json 零改动。

### 3.5 安全语义（v0.2 修订）

- **本机信任模型（精确表述）**：CONNECT 无 per-请求 token，安全边界 = 数据面默认绑定 `127.0.0.1`（`core lib.rs:90`）+ allowlist 默认空（全 403）。**默认配置下**不存在开放中继；若运维将数据面绑定为非回环（`PPROXY_LISTEN_DATA` 可覆盖，无既有告警），白名单域即成为无鉴权出口 → **新裁决 S-P2-数据面：启动时检测数据面绑定非回环且隧道已配置 → `tracing::warn!`（对齐 admin 面 `main.rs:98-102` 先例），文档写明风险**。
- **worker 侧防线（表述降级）**：Bearer token（SHA-256 比对）+ 仅 443 + `validHost` 黑名单（**best-effort**：十进制/hex/八进制 IP、IPv6 变体存在已知绕过形态，真实边界是 CF runtime 对私有目标不可达 + server 端 loopback/allowlist）。`/debug` 未鉴权返回配置状态 oracle、401 无速率限制、worker 日志记录完整目标 host → **accepted residual**（worker 零改动前提，记录于 §8.3）。
- **token 面（精确表述）**：不入 git、不入 SQLite、server tracing 不含 token/Authorization。已知明文落点（均为既有，非本 spec 引入）：`/api/tunnel/config` 响应体（admin 鉴权 + tailnet 边界）、`~/.pony/config.toml`（如配置）、init/deploy 的 stdout 提示。数据面新增要求：错误/日志不携带 worker reason 原文中的敏感信息（见 §3.6）。
- **TLS 端到端**：网关不终结 TLS，无解密能力，无 CA 责任面。
- **用量**：隧道连接全局计数（`tunneled` 日志语义），不进 per-token 用量表。已知权衡：流式推理可分钟级占用 256 并发配额中的 permit（accept 循环排队背压）；dev 单用户场景可接受，多用户部署需为隧道引入独立预算（演进项，不在本 spec）。

### 3.6 响应语义（枚举固定）

| 场景 | 响应 | `x-pproxy-reason` |
|------|------|-------------------|
| 隧道未配置（env 缺失/校验失败） | 403 `{"error":"connect_forbidden"}` | `tunnel_not_configured` |
| 目标 port ≠ 443 | 403 同上 | `port_not_allowed` |
| host 未命中 allowlist | 403 同上 | `no_tunnel_route` |
| WS 建连失败（网络类，含重试后） | 502 `{"error":"tunnel_failed"}` | `tunnel_failed` |
| worker 明确拒绝 `{"ok":false}` | 502 同上 | `tunnel_failed`（denied 不重试） |

响应体与 reason 头**内容固定为上述枚举**：不携带 tungstenite 错误文本、worker `reason` 原文、gate URL（防内部拓扑/token 相关泄露）；失败详情仅入 server 日志（tracing，不含 token/Authorization）。

### 3.7 doctor 隧道探针（阶段 2，零机密设计）

关键事实：本方案下 server **先 establish 成功才回 200**（hyper upgrade 前置），因此 doctor 对数据面发裸 `CONNECT oauth2.googleapis.com:443 HTTP/1.1` 并等待 `200 Connection Established`，即可验证 WS 建连 + token + worker ACL 全链路——**探针自身不需要任何 token/env**。
- 输入：数据面地址（复用 `derive_from_base` 推导 + `--data-plane` 覆盖，既有机制）；探针 host 默认 `oauth2.googleapis.com:443`，`--tunnel-host` 可覆盖。
- 结果矩阵：`200` → `[pass] tunnel probe`；`403 + tunnel_not_configured` → `[skip]`（服务端未启用隧道）；`403 + no_tunnel_route / port_not_allowed` → `[fail]`（打印 reason，提示 allowlist 配置问题）；`502` → `[fail]`（worker 链路问题）；连接失败 → `[fail]`。
- 不打印任何 token/Authorization（测试断言覆盖）。

### 3.8 被否决的替代方案

- **本地 MITM + 路径中继**：需 CA 信任链 + worker 泛化 host-in-path + 解密一切白名单流量的安全责任面。否决。
- **仅加 google 路径中继路由**：agy 端点硬编码，不可行（已实测）。
- **自读头 + 回灌适配器**（v0.1 方案）：与"普通 HTTP 路径完全不动"物理冲突（hyper 无法回灌已读字节），需自写 AsyncRead 适配器 + 残留字节归属定义，侵入性与风险均高于 hyper 原生 upgrade。否决（§8.1 #1）。
- **direct 直连回退**：§1.4 非目标（开放中继/SSRF 面 + 过度设计）。否决。
- **改动 worker**：worker 已具备所需能力，零改动是显式优势。

## 4. 任务拆解（阶段门禁）

| ID | 阶段 | 内容 | 验收门禁 | 依赖 |
|----|------|------|---------|------|
| T0 | 阶段 0 | spec + 3 路对抗审核 + 修订 | §8.1 记录完整，P0/P1 清零或裁决 | - |
| T1 | 阶段 1 | `connect.rs` + `gateway.rs` hyper CONNECT 拦截 + `main.rs`/`Cargo.toml` 接线 + S-P2-数据面 warn + 单测 + 双代码审核 | `cargo test -p pproxy-server` 全绿 + `cargo build --workspace` + 双审通过 | T0 |
| T2 | 阶段 2 | doctor 隧道探针段（零机密）+ 单测 + 双代码审核 | `cargo test -p pproxy-cli` 全绿 + dev 实跑 doctor 输出探针结果 | T1 |
| T3 | 阶段 3 | dev 部署：dev 上 `cargo build --release` + `systemctl restart pproxy`（env 已就绪）+ 实测 | §6.2 清单全过，含 `agy login` | T1,T2 |
| T4 | 阶段 4 | 收口：DELIVERY.md、§8.2 补代码审记录、commit/push、本地同步 | 仓库三方一致 | T3 |

每阶段完成后：更新 §0 状态表与断点（含未尽事项），再进下一阶段。

## 5. 风险与回滚

| 风险 | 缓解 | 回滚 |
|------|------|------|
| systemd 操作致服务起不来 | 只 restart，不改 unit/drop-in；改动前 `cp` 备份 `.pproxy.env` 与 `override.conf` | 恢复备份文件 + `daemon-reload` + restart。**禁止 `systemctl revert pproxy`**（本 unit 无 vendor 副本，revert 会删除 drop-in/unit，已确认 dev 存在 override.conf） |
| 隧道 token 配错/缺失 → 白名单域 502/403 | doctor 探针先行，reason 枚举可辨；`TunnelConfig::from_env` fail-closed | 数据面回滚 = 从 `.pproxy.env` 删除 `PPROXY_TUNNEL_*` + restart（注意联动：`/api/tunnel/config` 同步下发 null，桌面端自动配置失效——如桌面端在用需告知） |
| token 丢失需轮换 | dev 已有 token（§0 预核查）；若确需轮换：生成新 token → `wrangler secret put TUNNEL_TOKEN_HASH` → 更新 `.pproxy.env` → restart → 依赖旧 token 的桌面端需重新拉取配置（踢出效应需知情） | - |
| 二进制回滚 | 部署 = dev 上 `cargo build --release`（工具链已在） | `git revert <commit>` + 重新 build + restart |
| CF worker WS 时长/连接数限制 | OAuth/推理为短中连接；同通道桌面端已长期验证 | 无需 |
| `https_proxy` 全局注入下其他程序行为变化 | 现状非白名单域本就 403，本方案只增不减；无 direct 模式 | git revert |
| allowlist 过宽（如直接放 `com`） | 启动 warn 可疑条目；探针实测行为可核对 | 改 `.pproxy.env` + restart |
| agy 运行时流量不走 CONNECT（QUIC/HTTP3、内置 DoH） | **阶段 3 观察项**：OAuth 链路（AC3）不依赖运行时；运行时若失败用 `ss -u` 观察 UDP 443 出流确认；缓解手段（防火墙拦 UDP 443 逼降 TCP）列为预案，不在本 spec 范围 | - |

## 6. 测试与验收标准

### 6.1 自动化（阶段 1/2 门禁）

- `cargo test -p pproxy-server`：
  - `allowlist_match`：精确+子域命中；`notgoogleapis.com`、`googleapis.com.evil.cn` 拒绝；**`googleapis.com.hk` 拒绝（不含 alias 的区分性用例）**；大小写与末尾点归一；空 host 拒绝。
  - `TunnelConfig::from_env`：三者齐备+wss → Some；`ws://` → None（wss-only）；任一缺失 → None；只配其一 → None；allowlist 空段/trim 处理。
  - `handle_connect` 集成（本地 stub WS server 复刻 worker 协议：Text 首帧 JSON、`{"ok":true}`/`{"ok":false}`、Binary 透传、close 1008；tokio-tungstenite 0.24 `accept_hdr` 捕获 Bearer；stub 用 `ws://127.0.0.1` 免 TLS）：
    - 白名单 host + stub OK → `200 Connection Established` + TLS 无关字节透传往返；
    - 未配置隧道 → 403 + `tunnel_not_configured`；非白名单 → 403 + `no_tunnel_route`；port 8443 → 403 + `port_not_allowed`；
    - stub 返回 `{"ok":false}` → 502 且**不回 200**、无重试（断言 stub 仅收到 1 次连接）；
    - stub 不可达 → 502（重试后），**绝不回 200**（R4）；
    - 响应体/reason 枚举固定：502/403 不含 stub 返回的 reason 原文与 gate URL；
    - 普通请求回归：既有 `router.oneshot` 用例（鉴权/404/401）不变绿转红。
  - 非 443 预检、S-P2-数据面 warn 触发条件（函数级断言）。
- `cargo test -p pproxy-cli`：doctor 探针结果判定纯函数（200→pass；403+各 reason→skip/fail 矩阵；打印内容不含 token）。
- `cargo build --workspace` 通过。

### 6.2 手工实测（阶段 3，dev 服务器）

1. **AC1** `curl -sS -w '\nHTTP_CODE=%{http_code}\n' -x http://127.0.0.1:8899 https://oauth2.googleapis.com/token -d ''` → 期望 **HTTP 400 + JSON 含 `invalid_request`**（Google 真实响应；判据：非 `CONNECT tunnel failed`、非 403/502）。若 agy 运行时另需域，实测后按 §5 补 allowlist。
2. **AC2** `curl -v -x http://127.0.0.1:8899 -m 5 https://example.com` → stderr 中 CONNECT 响应行 `HTTP/1.1 403` 且含 `x-pproxy-reason: no_tunnel_route`（curl 默认不显示 CONNECT 响应头，必须 -v）。
3. **AC5** `pproxy doctor` → 新增 `tunnel probe` 段 [pass]；既有 route tests 全过（路径中继回归）。**AC6**（回归）= route tests + `/{token}/{route}/` 数据面探针 [pass]。
4. **AC3** `agy login` → Google 授权全流程成功；`agy` 推理调用观察（QUIC 预案见 §5）。
5. **AC4** dev 上 `cargo test --workspace` 全绿 + `cargo build --release` 成功。

## 7. 关键设计决策记录

| # | 决策 | 理由 | 备选与否决 |
|---|------|------|-----------|
| D1 | 复用 worker `/ws` 隧道，server 端移植桌面客户端逻辑（行为一致，配置访问重写） | 全链路桌面端生产验证；worker 零改动 | MITM（否决）；逐字移植（否决：HEAD 基线失配） |
| D2 | env 配置，config.json 零改动 | M3 先例；机密不入库；与既有 `TunnelProvision` 同源 | config.json 字段（如需热更新再议） |
| D3 | **无 direct 模式**：CONNECT 仅两态（隧道/拒绝） | 非回环绑定下 direct=无鉴权开放中继（含云 metadata SSRF）；dev 直连无价值；砍掉 45 行移植与测试面 | v0.1 的 `PPROXY_CONNECT_MODE`（删除） |
| D4 | allowlist 仅 suffix 匹配，不含区域别名 | 服务端面向 API 域；防止 `googleapis.com.hk` 类静默放行 | 桌面端全量语义（差异已测试钉死） |
| D5 | 隧道不进 per-token 用量 | CONNECT 无 token 语境 | 强绑 token 需改协议，超范围 |
| D6 | **hyper 原生 CONNECT upgrade**（RouterHyperAdapter 拦截 + establish 先行 + `hyper::upgrade::on` 透传） | 头解析/上限/超时/字节衔接全部由 hyper 承担；absolute-form 零风险；R4 语义（先建连后写 200）完整保留 | 自读头+回灌适配器（v0.1，否决） |
| D7 | 数据面 `gate_url` 强制 `wss://` | Bearer token 直上该 URL；管理面宽松校验不适用于数据面 | 沿用 tunnel.rs 宽松校验（否决） |
| D8 | **S-P2-数据面（新裁决编号）**：数据面非回环绑定 + 隧道已配置 → 启动 warn | 把 admin 面 S-P2-额外 先例扩展到数据面；信任模型支柱显性化 | 启动报错（过强，单机场景 warn 足够） |

## 8. 审核记录与采纳

### 8.1 Spec 审核（阶段 0，3 路独立，2026-08-28）

| Reviewer | 结论 | 关键发现 |
|----------|------|---------|
| architect | 有条件通过 | P0-1 自读头与 hyper 复用冲突；P1-1 移植基线 HEAD 不编译；P1-2 absolute-form 拦截会回归路径中继；P2-4 direct 过度设计 |
| security | 有条件通过 | P1-1 direct 模式开放中继/SSRF；P2-1 PPROXY_TUNNEL_* 双消费者（tunnel.rs/api.rs）spec 漏列；P2-2 env 载体应为 .pproxy.env 非 root600；P2-3 ws:// 明文面；P2-4 头读无超时 slowloris |
| 红队证伪 | 不通过 | P0-1 同 architect；P0-2 基线不编译；P1-1 env 已被 tunnel.rs 消费/接线大半既成；P1-2 token 溯源缺失；P1-3 systemctl revert 破坏性；P1-4 agy 运行时假设零证据 |

**采纳表**（重复意见去重；两票以上问题必须修复原则下全部处置）：

| # | 来源 | 问题摘要 | 处置 |
|---|------|---------|------|
| 1 | arch P0-1 / rt P0-1 | 自读头方案与 hyper 复用物理冲突；未评估 hyper 原生 upgrade | **采纳**：D6 路线更换，§3.1/§3.2 重写；absolute-form 不拦截；peek 分支删除；头超时由 hyper 既有 30s 承担 |
| 2 | arch P1-1 / rt P0-2 / sec | engine_tunnel.rs HEAD 不编译，"E2E 已验证/逐字一致"失实 | **采纳**：§2 注记基线失配；§3.3 改"行为一致移植+配置重写"；desktop 破损记上游债 |
| 3 | arch P1-2 / sec P1-1 / arch P2-4 | absolute-form 拦截回归路径中继；direct 模式开放中继+过度设计 | **采纳**：§1.4 明确不拦截 absolute-form；删除 direct 与 `PPROXY_CONNECT_MODE`（D3） |
| 4 | rt P1-1 / sec P2-1 | PPROXY_TUNNEL_* 已被 tunnel.rs/api.rs 消费（admin 下发），spec 漏列 | **采纳**：§2 资产表补行；§3.4 双消费者耦合如实记录 |
| 5 | rt P1-2 | token 溯源缺失（deploy 不生成、worker 只存 hash） | **采纳**：§0 预核查（dev 已有 token）；§5 轮换/踢出流程 |
| 6 | rt P1-3 / arch P2-5 | `systemctl revert` 破坏性（无 vendor unit + dev 已有 override.conf） | **采纳**：§5 回滚改为备份/恢复文件 + daemon-reload；部署方式 = dev cargo build |
| 7 | arch P1-3 / rt P2-2 / sec P2-4 | 头读无超时 slowloris | **采纳**：D6 下由 hyper `header_read_timeout(30s)` 覆盖，§3.2 写明 |
| 8 | arch P1-4 / rt P2-1 | doctor 探针数据源悬空（shell 拿不到 systemd env） | **采纳**：§3.7 零机密设计（裸 CONNECT 等 200，无需 token/env） |
| 9 | sec P2-3 | ws:// 明文面 | **采纳**：D7 数据面强制 wss:// |
| 10 | sec P2-2 | EnvironmentFile 口径（root600 错误、.pproxy.env 既有约定） | **采纳**：§3.4 对齐 .pproxy.env（dm 600）；drop-in 仅非机密 |
| 11 | sec P2-2 / rt | 非回环绑定无告警 | **采纳**：D8 新裁决 S-P2-数据面，列入 T1 |
| 12 | arch P2-3 / rt P3-3 | 非 443 端口 server 预检缺失 | **采纳**：§3.2/§3.6 `port_not_allowed` |
| 13 | arch P3-2 / sec P3-3 | denied 不重试；RETRY 语义 | **采纳**：§3.3（denied 立即失败，网络类重试 1 次）+ 测试 |
| 14 | arch P3-3 / sec P3-3 | 502/403 内容泄露面 | **采纳**：§3.6 枚举固定 + §6.1 内容固定性测试 |
| 15 | rt P2-4 / sec D4 | allowlist alias 通用 .com.xx 规则泄漏面 | **采纳**：D4 + 区分性测试（`googleapis.com.hk` 拒绝） |
| 16 | arch P2-6 / rt P3-1 / rt P2-5 | AC 判据不可操作（curl -v、AC 编号、400+invalid_request） | **采纳**：§6.2 重写 |
| 17 | arch P3-2 / rt P3-2 | GatewayState 测试构造点（3 处） | **采纳**：§3.2-4 列入 T1 |
| 18 | arch P3-1 | futures-util 无需新增 | **采纳**：§2 依赖行更正（仅 tokio-tungstenite） |
| 19 | rt P1-4 | agy 运行时 QUIC/DoH 零证据 | **采纳**：§5 观察项 + 预案；AC3 拆分 OAuth/运行时 |
| 20 | arch P2-1 / arch P3-6 | Semaphore 长连接分析；h2c 备忘 | **采纳**：§3.5 用量段；D6 下 h2c 问题消解 |
| 21 | sec P3-1/P3-2 | worker 防线高估、/debug 等既有暴露 | **采纳**：§3.5 降级表述 + accepted residual（§8.3） |
| 22 | arch P3-5 | allowlist 脏条目静默不命中 | **采纳**：§3.3 启动 warn |

### 8.2 代码审核（每阶段双审）

> 待填：阶段 1/2 的 reviewer-a（正确性/回归）/ reviewer-b（边界/失败路径/简化）结论与采纳记录。

### 8.3 安全专项

> security reviewer 对 §3.5 的挑战已并入 §8.1（#3/#4/#9/#10/#11/#13/#14/#21）。accepted residual：worker `/debug` 无鉴权、401 无速率限制、worker 日志记录完整 host、`validHost` 黑名单 best-effort——均既有且 worker 零改动前提不变。
