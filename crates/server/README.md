# pproxy-server

`pproxy` **守护进程**：一个进程同时提供三面——CONNECT 数据面、反向 API 网关、管理面 REST。
`pproxy serve` 会在同进程内起嵌入式引擎（`crates/engine`）；本 crate 额外承担管理面、监控与静态分发。

## 配置项

| 变量 | 作用 | 缺省 |
|---|---|---|
| `PPROXY_CONFIG` | 旧版 `config.json` 路径（含 worker 凭据/上游项） | `/etc/pproxy/config.json` |
| `PPROXY_LISTEN_DATA` | 数据面监听地址覆盖 | 空 = 用配置/引擎缺省（`127.0.0.1:8899`） |
| `PPROXY_LISTEN_ADMIN` | 管理面监听地址（tailnet 重绑用） | `127.0.0.1:8900` |
| `PPROXY_MAX_CONNECTIONS` | 网关并发上限（非法/0 回落） | `256` |
| `PPROXY_TUNNEL_GATE_URL` / `PPROXY_TUNNEL_TOKEN_DATA` / `PPROXY_TUNNEL_TOKEN` / `PPROXY_TUNNEL_ALLOWLIST` | 隧道端点 / 数据面令牌（优先）/ 下发令牌 / 白名单 | 空 = 不建隧道池 |
| `PPROXY_TUNNEL_POOL=0` | 关闭待命池（调试/受限环境） | 未设 = 开启 |
| `PPROXY_EDGE_KEEPALIVE=1` | 开启边缘保活 | 未设 = 关闭 |
| `PPROXY_YOUTUBE_PROXY` | YouTube / `googlevideo` 目标的专属上游（`http://` 或 `socks5://host:port`）；命中即分流，**单条失败回落常规隧道并打 WARN** | 空 = 关闭分流 |
| `PPROXY_RELAY_IDLE_TIMEOUT_MS` / `PPROXY_RELAY_ABSOLUTE_TIMEOUT_MS` | CONNECT relay 空闲 / 绝对生命周期上限（非法/0 回落） | `30000` / `21600000`（6 h） |
| `PPROXY_ALERT_WEBHOOK_URL` | 配额告警 webhook | 空 = 只落库不外发 |
| `PPROXY_DESKTOP_DIST_DIR` | `/dsk/` 桌面端安装包分发目录 | `/opt/pony-desktop-releases` |

## 语义

- **CONNECT 数据面**（`connect.rs`）：隧道待命池 `POOL_SIZE = 4`、建连尝试 `MAX_ATTEMPTS = 5`；
  allowlist 仅记日志（不作拦截门）；出口顺序由 `pproxy-transport` 按目标 host 决定；YouTube 目标经
  `PPROXY_YOUTUBE_PROXY` 直出，避免消耗原生 VPS 流量与 429 风险。relay 有**双向**期限（空闲 + 绝对），
  两者都可经环境变量缩短（契约与默认值见 `docs/ops/API.md`）。
- **反向 API 网关**（`gateway.rs`）：`auth_middleware` 路径模式优先（`/{token}/{route}/*path`）、header
  模式兜底（`X-Pony-Token`）；`/`、`/clash`、`/clash.yaml`、`/dsk/*` 不鉴权。absolute-form（明文 HTTP
  正向代理）请求在路径解析**之前**直接 `501 forward_proxy_unsupported`——否则 `/v1/models` 的 `v1`
  会被当成 route 名去鉴权，把"代理配错"伪装成"凭据错误"。
- **管理面 REST**（`api.rs`，`:8900`，`Authorization: Bearer <admin_token>`）：`/api/tokens`、
  `/api/tokens/:id`、routes CRUD + `/api/routes/:name/test`、`/api/usage`、`/api/health`、`/api/alerts`
  与 `/api/alerts/:id/read`、`/api/quota`、`/api/monitor/config`、`/api/tunnel/config`、`/dsk/:filename`。
  鉴权失败与缺失**同 401 同体**（不区分原因，防探测）。端点契约以 `docs/ops/API.md` 为准，本节只列面。
- **监控**（`monitor.rs`）：配额快照轮询 + 80% / 95% 越线告警落库 + 可选 webhook 外发。
- **隧道配置面**（`tunnel.rs`）：`/api/tunnel/config` 回传当前隧道 `url` 与数据面 `token`
  （未配置时两者为 `null`）。**它返回凭据本体**——所以该端点必须留在 Bearer admin 保护之下，
  这也是管理面默认只绑回环的原因之一。

## 限制

- **单节点**：状态在本地 SQLite（`PPROXY_DB` 或 `~/.pony/state.db`），没有跨节点共享账本，不提供水平扩展。
- **管理面默认只绑回环**：要远程管理必须显式改 `PPROXY_LISTEN_ADMIN`（典型做法是绑 tailnet 地址）。
  注意 `/api/tunnel/config` 会回传数据面令牌本体，管理面暴露面 = 凭据暴露面。
- **数据面不支持明文 HTTP 正向代理**：absolute-form 一律 501，没有"降级成 HTTP 代理"的开关。
- **CONNECT 仅 443**：其他端口在数据面就被拒，不属于可配置项。
- 配额计数与并发上限都是**进程内 + 本地库**口径；重启丢内存计数（库里的小时级用量仍在）。

## 扩展点

- 新管理端点：`api.rs` 加 handler + 路由，并在同一次提交更新 `docs/ops/API.md`（协议文档是契约家）。
- 新数据面行为：`connect.rs`（隧道/relay/分流）或 `pproxy-transport` 的路由函数，别在 gateway 里判出口。
- 新分发产物：`dsk.rs` 的目录约定 + `PPROXY_DESKTOP_DIST_DIR`；打包与 feed 校验在
  `.github/workflows/desktop-release.yml` 与 `scripts/`（见 `.meta/gates/instance-commitments.md`）。
- 新告警通道：`monitor.rs` 的 channel 抽象（webhook 只是其中一种实现）。
