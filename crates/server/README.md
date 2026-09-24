# pproxy-server

`pproxy` 本地 HTTP 反向 API 网关服务。

## 核心职责

- 本地代理服务端口监听（默认 `:8899`）。
- API 路径路由转发（`/{token}/{route}/*`）。
- Token 鉴权中间件、Gatekeeper 限流与 SQLite 计量统计。
