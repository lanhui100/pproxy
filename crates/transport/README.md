# pproxy-transport

`pproxy` 底层连接协议抽象与网络传输实现。

## 核心职责

- WebSocket 客户端/服务端会话封装与流传输。
- TLS/TCP 拨号与连接复用机制。
- 待命隧道池（TunnelPool）底层协议交互支持。
