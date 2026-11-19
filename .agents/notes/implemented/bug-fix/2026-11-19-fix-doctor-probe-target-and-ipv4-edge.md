# Agent Note: 修复 doctor CONNECT 探针 407 误报与跨境客户端 IPv6 丢包

Status: implemented

> 前置与关系：`2026-11-19-cli-probe-timeout-and-placeholder-fallback.md` 解决了 `derive_data_plane` 占位符污染与 2s 超时误报。
> 本条针对上线部署体检时发现的另外两处隐患：`pproxy doctor` CONNECT 探针因推导到非回环 Tailscale IP 触发服务端 407 鉴权拦截误报，以及 `EdgeClient` 跨境请求命中双栈 Anycast IPv6 路由黑洞导致的超时。

## Problem

在部署与健康自检过程中发现两处问题：

### 1. `pproxy doctor` CONNECT 探针误报 HTTP 407

执行 `pproxy doctor` 自检时，第 5 阶段探针失败：
```
[fail] CONNECT tunnel probe: oauth2.googleapis.com:443 → HTTP 407: HTTP/1.1 407 Proxy Authentication Required
```
根因：
- 本机配置了 `server = "http://100.95.193.103:8900"`（Tailscale 管理面 IP）以及 `data_plane = "http://127.0.0.1:8899"`；
- `crates/cli/src/main.rs` 因历史假设忽略了 `cfg.data_plane`，仅接受命令行显式 `--data-plane`；未提供时 `doctor.rs::connect_plane` 从 `base_url` 推导出 `http://100.95.193.103:8899`；
- 探针以无认证发送裸 CONNECT 请求，而服务端 `connect_auth_allowed` 为保证安全只对严格回环（`client_ip.is_loopback()`）豁免认证；Tailscale IP 属于外部地址，服务端合规拦截并返回 407。

### 2. 双栈 IPv6 跨境链路黑洞造成上游探测与路由建连超时

- 开发机具备公网 IPv6 地址，Linux 默认优先尝试 IPv6 连接；
- 在请求海外双栈 Anycast 节点（如 Cloudflare Anycast 的 `edge.ponygo.fun`）时，国内跨境 IPv6 存在高丢包与 MTU 黑洞，TLS 握手常在 Server Hello 阶段卡死直至 10s-15s 超时；
- 直连实测：IPv6 耗尽 15s 超时断连；强制 IPv4（`curl -4`）仅需 0.2s 即可完成握手并响应 HTTP/2 200。

## Decision

1. **`main.rs` 与 `doctor.rs` 探针目标推导与回环容错**：
   - 在 `crates/cli/src/main.rs` 中，若命令行未显式指定 `--data-plane`，但 `cfg.data_plane` 配置了有效的本地回环地址（包含 `127.0.0.1` / `localhost` / `[::1]`），优先作为探针数据面；
   - 在 `crates/cli/src/cmd/doctor.rs::tunnel_probe` 中，若向推导目标发起的裸 CONNECT 遭遇 407 且目标为非回环 IP，自动回落探测本地 `127.0.0.1:8899`；若回环通过则采纳，彻底消除 Tailscale IP 环境下的 407 误报。
2. **`EdgeClient` IPv4 强制绑定**：
   - 在 `crates/core/src/edge.rs::EdgeClient::new` 中，通过 `Client::builder().local_address(IpAddr::V4(Ipv4Addr::UNSPECIFIED))` 绑定 IPv4 本地地址，强制 socket 采用 `AF_INET`，避开 IPv6 跨境黑洞。
3. **连通性探测超时裕量提升**：
   - 在 `crates/cli/src/cmd/proxy_env.rs` 中，将正向探测超时由 5000ms 调整为 8000ms，对齐 `pproxy-transport` 的交互级拨号预算 `DIAL_TIMEOUT = 8s`，容忍跨境抖动。

## Alternatives considered

- **放宽服务端鉴权豁免网段（将 Tailscale `100.64.0.0/10` 纳入免密白名单）**：否决。Tailscale 节点可能来自不同设备，放宽非回环免密会破坏内网安全边界，使内网其它节点可未经授权利用代理隧道出海。
- **全局禁用宿主机 IPv6（`sysctl net.ipv6.conf.all.disable_ipv6=1`）**：否决。全局禁用会破坏 Tailscale 等依赖 IPv6 本地链路或回环协议的组件，在应用层通过客户端绑定 IPv4 才是最小侵入且受控的方案。
- **让探针带凭据执行 CONNECT**：否决。CONNECT 探针的设计初衷即为零机密探测（spec §3.7），探针不应依赖或暴露数据面数据 Token。

## Consequences

- `pproxy doctor` 无需任何额外参数即可直接通过所有测试（`10 passed, 0 failed, 1 skipped`），CONNECT 探针稳定返回 200。
- `EdgeClient` 反向路由测试时延大幅降低，`test github` 等外部上游从超时 10s 骤降至 < 1s（886ms）。
- `pproxy status` 在跨境网络抖动下对 Google (976ms) 与 GitHub (1404ms) 的正向出海探测更加稳健。
