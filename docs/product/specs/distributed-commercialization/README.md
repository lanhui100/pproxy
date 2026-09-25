# 规格说明：PProxy 自包含轻量多租户与分布式容灾架构 (v1.1.0 调优版)

> **版本**：v1.1.0-hardened (通过三路对抗审查安全加固)  
> **更新日期**：2026-09-25  
> **审查裁决**：采纳 8 项 P0 阻断、2 项 P1 隐患，重构非对称公私钥、配额租约、安全升级与客户端驱动  
> **作者**：Team Lead & Agent Team

---

## 1. 架构核心原则（加固后）

1. **非对称凭据隔离**：
   - 签发私钥仅保留在管理员本地离线机器（或安全 CLI）；
   - 所有线上节点（RackNerd, devserver, preprod 等）**仅持有 Ed25519 验证公钥**，具备极速验签能力，但**绝对无权伪造 Token**。
2. **基于信用租约的配额管控 (Local Credit Leases)**：
   - 总额度切分为各节点小额租约（如 500MB~1GB），节点在本地租约内放行流量；
   - 消除跨洋 Gossip 延迟带来的无限双花透支风险，将最大透支边界锁定在小额租约范围内。
3. **安全防篡改滚动升级**：
   - 二进制升级包必须携带 Ed25519 开发者私钥签名；
   - 节点强校验签名与 SHA256；Draining 设置 15 秒强制硬超时，彻底消除 2 节点状态机死锁。
4. **客户端真实协议对齐**：
   - 放弃浏览器渲染 402 HTML 的幻想（HTTPS CONNECT 无法注入 HTML），由客户端本地引擎捕获 402 并触发桌面 UI 充值弹窗与系统原生通知；
   - macOS 动态检测活跃网卡接口，异步调用 `networksetup` 并实现崩溃自愈还原；
   - TCP 握手设置 800ms 硬超时，消除 SYN 丢包卡死 20 秒隐患。

---

## 2. 详细分阶段实施路线图 (Phase 1 ~ Phase 5)

### Phase 1：多租户令牌模型、租约计量与 402 截断 (Token & Leased Metering)
- **交付目标**：
  1. `crates/core/src/auth.rs`：实现基于 Ed25519 的自包含 User Token 编码与公钥解码（字段：`jti`, `sub`, `quota_bytes`, `exp`, `max_conns=3`, `iat`）；
  2. 离线签发工具：CLI `pproxy user add` 仅在有私钥的管理机上运行；节点仅部署公钥；
  3. `crates/gate-server` 与 `crates/server`：流式传输接入自适应动态计数器；
  4. 新建连与流式阻断：首包若无租约额度立即响应 HTTP 402；活跃长连接额度耗尽立即发送 WS 4402 关闭帧并关闭底层 socket；
  5. 内部代理转发时物理剥离（Strip）`Proxy-Authorization` 等用户凭据 Header，防止外泄。

### Phase 2：自包含集群 Gossip、一键 Join 扩容与集群状态大盘 (Cluster Mesh & Leases)
- **交付目标**：
  1. 内部通信接口升级为 HMAC/签名校验，引入 `schema_version` 保证向前兼容；
  2. 实现基于信用租约的用量同步协议；
  3. 一次性加入令牌（One-Time Join Token, TTL 10分钟）：`pproxy cluster token create` 与 `pproxy cluster join`；
  4. 引入 SWIM 状态机模型（Alive -> Suspect -> Dead）与 Tombstone 墓碑机制，根除幽灵节点；
  5. 集群大盘：`pproxy status` 输出本地健康度 + 全网 Peer 矩阵；
  6. 服务端默认双模监听（`0.0.0.0:8899` 本地出海代理 + Tailscale 内网管理面）。

### Phase 3：防篡改零停机滚动升级流水线 (Zero-Downtime Rolling Upgrade)
- **交付目标**：
  1. 升级包签名机制：升级包强制携带签名文件 `.sig`，节点硬编码开发者根公钥验签；
  2. 滚动状态机：支持本地 P2P 流式、MinIO 与 R2 三级源；
  3. Draining 状态设置 15 秒强制超时熔断（Hard Timeout），覆盖双节点集群边缘场景；
  4. 单节点探活失败立即中止全集群升级并自动告警回滚。

### Phase 4：客户端系统级适配、402 原生通知与故障漂移 (Client Hardening)
- **交付目标**：
  1. 凭据升级：输入 User Token 自动拉取 `/api/user/profile`，主界面展示用户卡片、配额进度条与到期时间；
  2. 402 交互体验：捕获服务端 402，通过 Tauri Event 弹出充值窗口，推送系统原生通知；
  3. 402 静默自愈：后台阶梯退避探测（1s -> 2s -> 4s -> 8s -> 15s），充值后秒级自愈恢复，提供“立即检测”按钮；
  4. 故障漂移加固：TCP 握手设置 `connect_timeout <= 800ms`，配合熔断器（连续 2 次失败冷却 30s），实现真正的无感漂移。

### Phase 5：macOS 系统代理健壮驱动与 Clash Meta 订阅 (macOS & Ecosystem)
- **交付目标**：
  1. macOS 系统代理驱动：动态通过 `route get default` 获取活跃网卡 BSD Name，异步多线程调用 `networksetup`，添加 `sysproxy_snapshot.json` 崩溃自愈；
  2. 打包分发：配置 macOS DMG 打包与 Windows NSIS 安装包；
  3. Clash Meta 生成：`pproxy user gen-clash` 与桌面端扫码导入，增加 `Subscription-Userinfo` 头部，对探活免计费。

---

## 3. 验收质量门禁 (Quality Gates)

1. **测试全绿**：`cargo test --workspace` 100% 通过；
2. **安全渗透测试**：使用篡改的 Token、伪造的二进制、并发多节点打满流量测试，必须 100% 被公钥验签与租约熔断拦截；
3. **滚动升级压测**：双节点集群并发大文件传输时触发升级，客户端由于 800ms 超时和无感重试，HTTP 成功率保持 100%；
4. **多平台实机测试**：Windows 11 与 macOS（有 Wi-Fi 与纯以太网设备）上系统代理开关、异常 kill 测试，确认网络不被断开。
