# Agent Note: distributed-quota-credit-leases-and-metering

Status: proposed

## Problem

在多节点分布式部署与桌面客户端商业化场景下，多租户令牌配额（User Token Quota）计算存在严重失真与口径漂移，链路上存在以下结构性缺陷（关联 `docs/product/specs/distributed-commercialization/FOLLOW-UPS.md` #F-01 / #F-02 及 `crates/gate-server/src/lib.rs`、`desktop/src/views/DashboardView.vue`）：

1. **桌面端口径严重失真**：`desktop/src/views/DashboardView.vue:215-233` 使用 `getUserBaselineBytes(sub, currentTotal)`，将本机所有出口（CF + Vercel + RackNerd + Upstream）的全局累计流量增量直接算作当前租户已用额度，且基线仅写入一次永不重置。多用户切换、重置缓存、多进程共用机器时，已用额度产生严重错配。
2. **边缘节点内存无状态与双计费缺陷**：`crates/gate-server/src/lib.rs` 的 `user_used_bytes` 为节点内存 `DashMap`，进程重启即归零；WS 隧道与 TCP CONNECT、`/api/proxy` 口径割裂；WS 流式双向转发时，上行帧与下行帧分别累加进同一 `used_bytes`（双向合计）但未向客户端与 API 声明；超额取消时先 `fetch_add` 未投递字节且未回滚，产生"用户被计费但帧被丢弃"的现象。
3. **跨节点信用租约（Local Credit Leases）未兑现**：`UserTokenClaims` 中的 `lease_bytes` 字段（`crates/core/src/auth.rs:21`）仅停留在 Token 载荷声明，各出海节点间无租约切片预占、无中心/对等节点配额扣减与对账，用户在节点 A 消费后节点 B 无法感知，造成无界双花透支窗口（#F-01）。
4. **并发连接与撤销反熵穿透**：`user_active_conns` 本地计数并在网关被 `max(10)*4` 软性放宽（`lib.rs:358`），多节点无法协同限制 `max_conns`（#F-02）；撤销仅依赖单机文件与本地内存，缺乏签名 Tombstone 与跨节点广播反熵（#F-03）。

## Proposal

根据三路对抗审核（安全一致性、正确性口径、可用性运维）的结论，重构分布式配额与计量体系，将"唯一真相源"由客户端推导收敛至服务端持久化与信用租约协同管控：

### 1. 唯一计量口径与计费范围
- **计费生命周期 Scope**：`quota_bytes` 严格绑定 Token 签发期 `[claims.iat, claims.exp]`。自然月/天仅作为展示统计窗口，不改变 Token 硬配额熔断边界。
- **双向合计计费与分项透出**：明确 `used_bytes = bytes_up + bytes_down` 合计计费规则，并在 `/api/user/profile` 与数据持久层明确透出 `bytes_up` 与 `bytes_down` 明细；计费逻辑调整为"先确保数据发送成功再原子计费"，若遭遇配额耗尽熔断，取消帧字节做原子回滚（`fetch_sub`）或在发送前精确判定，杜绝未发先扣。
- **全入口收口**：WS 隧道、HTTP CONNECT 隧道及 `/api/proxy`（需携带验签后的用户身份凭据）统一接入同一 `sub` 的计量管道。

### 2. 局部信用租约（Local Credit Leases）落地
- **租约切片申请与兑现**：出海节点（如 gate-server / 边缘网关）启动或本地租约耗尽时，按 Token 中的 `lease_bytes` 向对账底座（或管理面）原子申请小额租约切片（例如 500MB~1GB），并在本地受限额度内放行。
- **有界透支控制**：全局最大透支量锁定为 `Σ(未过期节点租约余额)`，彻底杜绝无界双花。
- **租约生命周期与租约回收**：租约附带全局单调 Epoch、Grant Timestamp 与 TTL。若节点意外崩溃或宕机，未消费租约在中央底座对账 TTL 到期后自动回收释放；节点重启后上报实际消耗量，余额失效重新领租。
- **高可用分级降级**：
  1. 底座不可达但本地仍有剩余租约：持续放行；
  2. 本地租约耗尽且底座不可达：进入 60 秒宽限期或允许 1 次应急宽限切片（带系统警报），超时后返回 402；
  3. 保留 `PPROXY_LEASE_FAIL_OPEN=1` 运维应急旁路开关。

### 3. 持久化存储与写入解耦
- **SQLite 表结构演进**：新增 `user_usage` 表，按 `(sub, token_jti, window_start)` 记录绝对累计水位，包含 `bytes_up`、`bytes_down`、`updated_at`，杜绝重复增量叠加。
- **热路径零锁**：数据面热路径维持原子内存计数（`AtomicU64`），由独立异步任务批量队列化持久化，并引入 SQLite `busy_timeout` 退避重试与水位标记（Watermark），防止多进程并发死锁。

### 4. 客户端与管理面契约统一
- **客户端逻辑修正**：彻底删除 `desktop/src/views/DashboardView.vue` 中的 `getUserBaselineBytes` 假基线计算；改为轮询服务端 `GET /api/user/profile`，严格消费服务端验签后返回的 `used_bytes`、`quota_bytes`、`expire_at` 及分项数据。
- **Admin 豁免收紧**：废除基于 `name.starts_with("admin_")` 的前缀判定，严格以 `claims.role == "admin"` 判定管理员身份；管理员在 Profile 中标记 `unmetered: true`，用量保持客观记录但不执行 402/4402 熔断。
- **并发与撤销安全收敛**：删除网关层 `max(10)*4` 并发放宽代码，引入基于 UID 亲和锚点或租约槽位的连接上限管控；撤销指令引入管理端私钥签名 Tombstone + 单调时间戳，杜绝旧状态覆盖复活。

## Alternatives considered

- **A. 纯中心化实时扣减（每请求/每连接回库 RPC 强一致）**：
  - *分析*：跨洋出海延迟通常达 150~300ms，每个连接握手或帧传输同步查询中心库将严重破坏代理吞吐量与连接建立速度，且中心单点故障会导致全网瘫痪。
  - *裁决*：否决。维持局部信用租约（Local Credit Leases）方案，以局部小额（500MB）透支换取完全本地化的低延迟放行。
- **B. 客户端本地基于网关响应头对账（Client-Assisted Metering）**：
  - *分析*：由客户端自行统计上报或从网关响应头累加。客户端环境不可信，存在逆向、多端并发数据撕裂、篡改 LocalStorage 等欺诈风险。
  - *裁决*：否决。服务端必须是唯一权威真相源，客户端仅作只读呈现。
- **C. 完全依赖去中心化 Gossip 广播全量累加事件**：
  - *分析*：Gossip 网络事件到达存在乱序与延迟，网络分区时节点无法确认全局总额度，仍然无法杜绝并发透支，且消息风暴开销大。
  - *裁决*：否决。选择"底座集中对账核发切片 + 节点本地租约消耗 + 撤销 Tombstone Gossip 广播"的组合模式。

## Acceptance criteria

1. **单节点计量与重启持久化**：
   - 客户端经由 WS 隧道、HTTP 代理传输指定大小文件后，服务端 `user_usage` 表记录的字节数与真实传输量完全对齐（误差 ≤ 单帧协议开销）；
   - 网关进程强杀（SIGKILL）并重启后，`/api/user/profile` 返回的 `used_bytes` 能够从 SQLite 绝对水位准确恢复，不发生配额归零重置。
2. **多节点信用租约透支拦截**：
   - 设立总额度 8GB 的令牌，在 Node A 与 Node B 分别发起高并发下载；
   - 两节点各自基于 1GB 租约逐步放行并消耗，当两节点累计用量达到 8GB 时，后续请求被准确返回 HTTP 402，活跃长连接收到 WS 4402 帧并关闭底层 Socket，全局透支量严格不超过 1 个未消耗租约切片。
3. **桌面端大盘口径一致**：
   - 桌面端删除基线推导逻辑后，大盘显示的已用额度、剩余天数与 `/api/user/profile` 完全一致；多租户账号切换后额度立即对齐，无跨账号流量污染。
4. **单机兼容与安全防护**：
   - 未组网的单机部署默认降级为本地持久化配额模式，不依赖集群管理面；
   - 普通租户伪造 `name="admin_attacker"` 无法绕过配额熔断；
   - 令牌执行 `revoke` 后，全集群在 ≤1 个同步周期内返回 HTTP 401 阻断。

## Risks

- **冷备与网络分区时的可用性风险**：当边缘节点与管理底座发生跨洋长久失联时，租约耗尽可能导致正常用户被阻断。*缓解策略*：设置本地宽限时间（60s）与预分配缓冲租约，并支持运维开关 `PPROXY_LEASE_FAIL_OPEN=1` 紧急放行。
- **SQLite 频繁写并发锁瓶颈**：高并发连接如果直接同步写入 SQLite 将触发 `SQLITE_BUSY`。*缓解策略*：在内存中使用 `DashMap` + `AtomicU64` 进行秒级聚合，由单一写线程批量入库，严格保证数据面热路径零 I/O 阻塞。
- **在途长连接计量跨期归属**：若用户长连接跨越租约或窗口刷新边界，可能出现旧连接计入新周期或漏计。*缓解策略*：长连接在切片用量耗尽时触发主动续租，按实际产生流量的时间戳分段结算入库。
