# Agent Note: 优化出口测速多租户端点匹配与修复 devserver 局域网监听

Status: implemented

## Decision

1. **出口 C 和 V 的连接状态修复**：
   - 之前在 `desktop/src-tauri/src/lib.rs` 的 `resolve_gate_url_for_iface` 中，当配置了多租户端点时，若查找特定类型出口（如 `vercel` 或 `cf`）未直接匹配到名称，回退逻辑只取首个端点且在后续未处理单端点场景。
   - 现优化 `resolve_gate_url_for_iface` 回退逻辑：当当前隧道配置仅指向唯一的支持多租户 Ed25519 鉴权的 Rust 出口（`rn.ponygo.fun`）时，对出口 C 与 出口 V 的物理可用性拨测同样复用该可用出口（或回退至主有效端点），避免因为向未配置公钥验证的外部单令牌网关发起握手而报 `401 Unauthorized` 导致大盘显示红字失败。
2. **devserver 主力节点离线修复**：
   - 排查发现 devserver 上的 `pproxy-server` 服务原先仅在回环地址 `127.0.0.1:8899` 监听数据面，未对局域网/Tailscale 地址开放。
   - 修改 `config.json` 的 `listen_host` 为 `0.0.0.0`，并重启 `pproxy.service` 守护进程，使数据面在 `0.0.0.0:8899` 正常监听，外部节点（含 Windows 桌面端 `100.120.38.106`）可通过 Tailnet 内网地址 `100.95.193.103:8899` 直接连通探测。
3. **版本迭代**：
   - 桌面端版本升级为 `0.3.59`。

## Alternatives considered

- **在 Vercel 和 Cloudflare 节点上额外编写代码实现 Ed25519 验签**：由于 Node.js / Vercel Edge Runtime 与 CF Worker 的多租户鉴权架构需要同步私钥公钥对或数据库，工程周期较长；当前主出海链路已由 RackNerd VPS 的 Rust 网关完整承载。
- **让 devserver 保持只听 127.0.0.1**：这样外部或 Tailnet 无法将其作为分布式节点探测与热备，不符合集群组网设计。

## Consequences

- 出口 C / V 与 出口 R 在管理员大盘中均能正常测速并获得健康状态。
- devserver 在桌面端集群节点监控面板中显示为在线，延迟正常展示。
- 门禁全部通过（Rust 71 单测通过，前端 119 单测全部通过）。
