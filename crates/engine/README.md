# pproxy-engine

`pproxy` 出海代理引擎与待命连接池管理器。

## 核心职责

- HTTP/HTTPS CONNECT 隧道协商与转发。
- 动态 WebSocket 连接池（TunnelPool）生命周期维护（维持 1 RTT 待命连接）。
- 出口节点故障倒换与分流策略调度。
