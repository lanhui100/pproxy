# Agent Note: vless-standard-protocol-ingress

Status: proposed

## Problem

gate（`deploy/cf-gate-worker/worker.js`）入站是**私有 WS 桥协议**（`/ws` + Bearer
sha256 鉴权 + 首帧 `{host,port}`）。第三方客户端（Clash/v2rayN/Shadowrocket）只能走
pproxy 自己的 engine/桌面端连接，**无法标准直连 gate**。B015 解决了"订阅对准 gate
域名"，但 Clash 直连依赖 gate 开放标准协议入站，否则隧道模式只是"生成一个连不上的
配置"。需求：评估 gate 增加 **VLESS-WS 入站**，把 Ed25519 配额令牌映射成 UUID 鉴权，
让第三方客户端直连 gate 且配额/吊销体系保留。

## Proposal

在 gate worker 增加 VLESS-WS 入站（`/vless` 路径），与既有私有 `/ws` 通道并存：

1. **协议**：VLESS over WebSocket（`wss://<gate域名>/vless`，TLS 由 CF 边缘终止，
   WS 内为 VLESS 帧：版本/命令/目标 host:port + UUID 鉴权）。
   - 支持 TCP 转发（443-only，沿用 ALLOWED_PORTS 语义）；UDP 不入 v1（Workers 无
     UDP 直连，与现有隧道一致）。
2. **鉴权映射**：Ed25519 配额令牌（`usr_live_*`）→ 确定性地派生 UUID：服务端不存
   UUID 表，用令牌的 Ed25519 验签密文/哈希映射——`UUID = 令牌的 HMAC-SHA256(域盐)`
   前 16 字节格式化。客户端（第三方）用**代理 UUID**（由授权码离线导出），gate 用
   同一派生函数还原令牌 → 校验未撤销 → 配额按令牌记账（复用现有 token 体系）。
   - 吊销热更新：撤销后 UUID 派生命中黑名单 → 拒绝（与现有 revoke 链路同源）。
3. **保留私有通道**：`/ws` 原样不动（桌面端/engine 继续用，零迁移成本）；
   `/vless` 作为第三方入口并存。
4. **指纹面控制**：配合 B011 伪装页——非 `/vless` 路径仍是 nginx 欢迎页；`/vless`
   的 WS 升级失败（坏首帧/鉴权失败）返回 404 而非协议错误（与隧道一致，不暴露
   "这是代理"）；VLESS 特征串运行时拼装。
5. **配置**：`VLESS_ENABLED`（默认关闭，显式开启）；`ED25519_VERIFYING_KEY` 复用
   现有多租户验签公钥注入；开启时 `/debug` 暴露 `vless: true/false`。
6. **客户端侧**：B015 的 clash 订阅增加 VLESS proxy 类型分支（当 VLESS_ENABLED 且
   用户用 UUID 授权码时），或提供 `pproxy authority export vless://` 导出。

## Alternatives considered

- **A. 入站改标准 Trojan 而非 VLESS**：VLESS 无加密开销、客户端生态最广（clash.meta/
  v2rayN/Shadowrocket 全支持），Trojan 需额外 TLS 密码学面；选 VLESS。
- **B. 直接暴露 XHTTP/gRPC**：xHTTP 需要 HTTP/2 长连语义，Workers 平台支持弱、
  社区验证少；gRPC 与 CF 边缘 Host 头限制耦合；VLESS-WS 是最成熟路径。
- **C. UUID 直接用令牌字符串（不做派生）**：令牌含 Ed25519 签名信息，直接当 UUID
  会泄露鉴权材料且格式非法（UUID 需 16 字节）；派生后 UUID 可公开、凭吊销表回收。
- **D. 由配额的 UUID 常驻表（KV/存储维护映射）**：引入外部状态、吊销需同步表；
  确定性派生零状态、与现有"无状态验签"架构一致。

## Acceptance criteria

- 开启 `VLESS_ENABLED` 后，Clash Meta/v2rayN 用导出的 `vless://` 授权码可直连
  `wss://<gate域名>/vless` 并出海（真实协议端到端，靠部署后 review）；
- 撤销某令牌后，其派生 UUID ≤1 心跳周期被 gate 拒绝（401/关闭）；
- `/ws` 私有通道行为零变化（桌面端回归）；
- 禁用 `VLESS_ENABLED` 时 `/vless` 返回伪装页（无协议痕迹）；
- 单测非零退出：UUID 派生确定性/吊销黑名单命中/首帧解析（node 对 worker 模块直测）。

## Risks

- Workers 对 WebSocket 长连接与并发限制：VLESS-WS 长会话同现有 `/ws`，受同平台
  额度约束（B012 gate_cf 观测已覆盖）；
- 协议解析代码量 + 指纹面变大（对比私有桥）；B011 伪装页为前置，缺失时不开 VLESS；
- 第三方客户端版本差异（VLESS UUID 鉴权字段），v1 只验 `uuid` 字段、忽略扩展；
- 本项是架构级大改，**先方案（本 note）后实施**；实施拆独立 backlog 项（B014），
  不与 B010-B013 混排。