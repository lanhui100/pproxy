# Agent Note: clash-subscription-gate-alignment

Status: implemented

## Problem

`pproxy clash` 生成的订阅（`~/.pony/clash.yaml`）server 固定为**局域网 IP + HTTP 代理**
（`server: <lan_ip>:8899`，手机连本机）。配置了 gate 隧道（`PPROXY_TUNNEL_GATE_URL`）
的场景下：gate 域名轮换（CF 风控/SNI 阻断，见 CF-RISK-SOP §4）后，订阅里的 server/HOST
仍是旧局域网 IP，第三方客户端无法直连 gate，只能手改配置——违背"改 gate 域名后重新
生成的订阅无需手改 HOST 即可用"。

## Decision

`pproxy clash` 增加**隧道直连模式**（B015）：配置了 `tunnel_gate_url` 时，订阅自动
对准 gate 域名：

1. **配置派生**：新增 `gate_host_from_config`——从 `PPROXY_TUNNEL_GATE_URL`（支持
   `wss://host[:port]/ws`、`wss://host[:port]`、`host:port`、多端点逗号分隔取第一个）
   提取纯 host；无配置返回 None，行为回落局域网 http 模式（现状不变）。
2. **隧道 YAML**：`generate_clash_yaml_tunnel` 生成 ws+tls 代理——`server=<gate 域名>`
   `sni=<同域名>`（HOST/SNI 自动对准）、`ws-opts.path=/ws`（对齐 gate worker 路径）、
   鉴权经 `ws-opts.headers.Authorization: Bearer <token>`（对齐 gate /ws Bearer 协议）。
3. **订阅 URL**：隧道模式输出 `wss://<gate域名>/ws`（二维码/URL 导入均可用）；
   使用提示按模式区分（隧道 vs 局域网）。
4. **行为保持**：未配置 `tunnel_gate_url` 时 YAML/URL/提示与 B015 前完全一致；
   `generate_clash_yaml` 签名未变（内部委托统一实现）。

## Alternatives considered

- **A. 订阅 server 继续用局域网 IP，靠"域名映射"让客户端解析**：落选。手机 Clash 在
  公网（非同一 WiFi）时局域网 IP 不可达；gate 隧道本就是公网端点，直接用 wss:// 直连
  更简单可靠。
- **B. 让用户每次手改订阅的 HOST/SNI**：落选。正是本决策要消灭的痛点；域名轮换后
  重新执行 `pproxy clash` 即得新订阅，零手改。
- **C. 隧道模式默认开启、覆盖局域网模式**：落选（负向）。未配置隧道的主机（纯局域网
  模式）若强行 wss 直连会不可用；必须显式配置 `tunnel_gate_url` 才切隧道模式，否则
  保持现状——"默认行为不变"纪律。
- **D. 把 gate 域名硬编码进 clash 模板**：落选。域名是部署态配置（轮换对象），硬编码
  违反"源码禁止硬编码端点"审计整改；必须从配置派生。

## Consequences

- 配置了 gate 隧道的环境，`pproxy clash` 订阅自动对准 gate 域名，域名轮换后重新生成
  即生效（免手改 HOST/SNI）；
- 未配置隧道的行为与 B015 前完全一致（局域网 http 模式）；
- 鉴权语义对齐 gate `/ws` 协议（Bearer），第三方 Clash 客户端可直连 gate；
- 机械验收：`cargo test -p pproxy-cli clash`（5 用例，含 tunnel 模式与端点解析）非零退出；
- 真机 Clash 直连验证靠部署后 review（wss 端点需真实 gate 在线）。
