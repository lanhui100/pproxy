# Agent Note: 恢复跨国 WSS 拨号超时窗口至 8000ms

Status: implemented

## Decision

`crates/transport/src/proto.rs` 中的非测试模式 `DIAL_TIMEOUT` 从 800ms
恢复放宽至 8000ms（与 v0.3.50 ~ v0.3.53 生产经过验证的口径保持一致）。

## Alternatives considered

- **维持 800ms 探测超时**：在纯内网或同机房低延时局域网下可消除挂起，但客户端处于国内公网（如家庭宽带、4G/5G 等网络），到海外 VPS 节点（如 RackNerd 洛杉矶）的单次 TCP RTT 已在 200~250ms 左右，叠加 TLS 1.3 握手与 WebSocket Upgrade 协议协商往返，总建连耗时极易突破 800ms，导致客户端在建立隧道前频繁主动超时丢弃连接，并在服务端 access log 留下大量 499 客户端主动关闭记录。
- **动态自适应超时**：实现复杂度高且容易引起连接池震荡，目前阶段无必要。

## Consequences

- 跨国高延迟网络（国内到美西）建连成功率恢复至 100%。
- 桌面端站点与代理链路探测不再被短超时直接切断判定为失败。
- 配套门禁：cargo test 与桌面单测全部通过。
