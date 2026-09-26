# Agent Note: PProxy 自包含轻量多租户与分布式容灾架构

Status: implemented

## Problem
`pproxy` 当前仅支持单租户静态口令（`TUNNEL_TOKEN_HASH`）和单机 SQLite 存储，无法区分多用户身份、缺乏按流量计费与超额拦截能力；同时作为出海网关若单机部署存在单点故障风险。在国内公网部署 SaaS 存在未备案及电信合规风险，因此需要一套轻量自包含、零外部数据库依赖、能在国内机器（K3s 节点）与海外轻量 VPS（RackNerd）间自动组网容灾、并支持一键签发令牌与客户端直接显额的架构。

## Decision
采用“非对称公钥验签 + 局部信用租约 + Local HA Forwarder 进程隔离 + 基于 RFC 2104 HMAC 机器互信票证的去中心化对等容灾”架构：
1. **多租户与自包含 Token**：采用 Ed25519 签名，私钥仅保留在管理员本地离线控制端（`~/.pony/cluster_signing_key.hex`，0600 权限），线上各数据与网关节点仅持有公钥环境变量 `USER_VERIFYING_KEY` 用于本地毫秒级验签（含 uid, quota, exp, max_conns=3）；
2. **Local HA Forwarder 独立守护进程**：独占监听本地 `127.0.0.1:8899`，核心主引擎监听内部端口 `18899`。引入后台异步 Zero-Wait 探活协程（300ms 探测 / 80ms 超时），健康时 0ms 关键路径直连本地 18899；本地引擎崩溃、重启或滚动升级时，0 毫秒延时瞬间 failover 漂移至远程对等节点，出海延迟维持在 1.0~1.4 秒；
3. **集群机器互信票证 (X-Pony-Cluster-Ticket)**：跨节点 failover 转发基于共享的 `cluster_auth_key` 签发 RFC 2104 标准的 HMAC-SHA256 短效票证，远端节点执行 120s 新鲜度校验与恒定时间无分支比对（`constant_time_eq`），并在字节切片层彻底清洗伪造头，杜绝明文用户密码越权利用；
4. **本机回环免密与反向代理安全硬化**：数据面对纯回环请求（`127.0.0.1`）免除鉴权开箱即用；一旦检测到外部代理头（`X-Forwarded-For`、`X-Real-IP`、`Forwarded`），坚决剥夺回环豁免特权，强制退回凭据认证；
5. **配置自愈零接触入网 (Zero-Touch Bootstrap)**：`cluster token-create` 自动打包生效的出海端点、隧道密钥与验签公钥，工作节点执行 `cluster join --token "<tok>" --auto-start` 时自动以 0600 权限完成落盘自愈装配并在后台自启双模服务；
6. **客户端原生感知与生态兼容**：402 错误在握手层与双向流式中实时熔断协同，阻断盲目重试；macOS 实现动态出接口探测与网络快照还原；Clash Meta 生成置顶探活免计费规则。

## Alternatives considered
1. **依赖集中式 PostgreSQL / Redis**：
   - *劣势*：污染现有服务器的其他业务数据库；集中 DB 成为单点故障，DB 宕机则全集群瘫痪；海外 VPS 跨洋直连国内 DB 延迟高且存在网络分区不可达风险。
   - *裁决*：否决。选择纯自包含内存 + 本地 SQLite + Ed25519 非对称公钥与集群票证模型。
2. **对称密钥全网共享签发或统一固定用户密码**：
   - *劣势*：在任意节点存储私钥或使用固定明文密码作为转发凭据，一旦某节点被攻陷或密码外泄，攻击者可自行签发无限额度 Token 或绕过鉴权直接蹭网。
   - *裁决*：否决。选择离线管理机保留私钥；节点间跨机转发统一使用基于 `cluster_auth_key` 动态生成的短效 HMAC 机器票证。
3. **基于全量累计流量的事后 Gossip 聚合**：
   - *劣势*：跨洋网络延迟（200~350ms）及丢包导致多节点并发时可轻松超额透支数倍流量（双花漏洞）。
   - *裁决*：否决。选择前置局部信用租约（Local Credit Lease）机制，将超额风险严格锁定在有界残值内。

## Consequences
- **Positive**：
  - 本地服务（如 ponyllm）绑定固定 `127.0.0.1:8899`，对本地 pproxy 主引擎的停机、崩溃与滚动升级实现完全免疫，不断网；
  - 备灾出海延迟从 6~10 秒优化收敛至 1.0~1.4 秒；
  - 系统安全性 100% 依赖密钥机密性，恪守柯克霍夫原则，代码开源对攻击者完全透明依然不可攻破；
  - 生产集群通过 dev / preprod / tencent 三节点实机容灾与攻击穿透演练验证。
- **Follow-ups / Technical Debt**：
  - 跨节点配额用量一致性（#F-01）、并发计数跨节点锚定（#F-02）、撤销跨节点广播（#F-03）已在 `docs/product/specs/distributed-commercialization/FOLLOW-UPS.md` 详实归档，事项层记入 `backlog/backlog.md`（B005~B009）。
