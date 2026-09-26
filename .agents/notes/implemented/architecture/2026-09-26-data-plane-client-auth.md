# Agent Note: pproxy 非回环数据面客户端鉴权

Status: implemented

## Problem

pproxy 的本地 HA forwarder 在非回环监听地址（`0.0.0.0:8899`）上承载 CONNECT 和普通 HTTP 代理。旧版 forwarder 仅签发集群节点间的内部票证，不验证客户端身份；非回环监听因此会形成匿名开放代理，无法安全地被 k3s 多租户集群共享。2026-09-26 红队复核确认腾讯节点数据面未鉴权，`pproxy-host` Service/Endpoints 已回滚下线（见 `deploy/pproxy-service.yaml` 顶部 BLOCKED 注释），ponyllm 网关的 antigravity provider 因 `proxy` 指向失效出口而直连 Google OAuth 端点，全部 10 个 key 刷新失败，模型 `gemini-3.8-flash-high` 报 502 `upstream_auth_failed`。

## Decision

为 `LocalHaForwarder` 增加可配置的客户端数据面令牌，并落地到腾讯节点与 k3s 集群：

1. **客户端令牌机制**（`crates/core/src/ha_forwarder.rs` + `crates/cli/src/main.rs`）：配置令牌后，forwarder 在转发前检查 `X-Pony-Token` 或 `Proxy-Authorization: Basic` 中的凭据（password 与令牌常量时间比较）；缺失或不匹配返回 `407 Proxy Authentication Required` 并关闭连接。集群票证 `X-Pony-Cluster-Ticket` 只允许远端集群节点之间的 HMAC 票证，**不能替代**客户端令牌。CLI 新增 `--client-token` 与 `--allow-loopback` 参数；独立 forwarder 进程从参数或 `PPROXY_CLIENT_TOKEN` 环境变量读取（serve 派生时继承 env）。
2. **腾讯节点部署**（100.105.241.39，tailscale）：`pproxy serve --lan` 由手动孤儿进程改为 systemd unit（`/etc/systemd/system/pproxy.service`，Restart=on-failure）持久化托管；unit 注入 `PPROXY_CLIENT_TOKEN=<48-hex>`，服务重启后 ha-forwarder 自动继承并强制鉴权。二进制升级为含鉴权的新构建（旧版二进制已备份 `~/.local/bin/pproxy.bak.*`）。
3. **k3s 出口重建**（namespace `ponyllm`）：重建无 selector 的 `pproxy-host` ClusterIP Service + Endpoints → `100.105.241.39:8899`（符合 `deploy/pproxy-service.yaml` 契约：不得加 selector，否则 Endpoints Controller 会接管删除手工地址）。
4. **ponyllm 网关配置**：`ponyllm-config` secret 中 antigravity provider 的 `proxy` 由指向 pod 自身回环的失效值改为 `http://ponyllm:<TOKEN>@pproxy-host.ponyllm.svc.cluster.local:8899`（reqwest `Proxy::all` 解析 URL userinfo 自动生成 `Proxy-Authorization: Basic`，password=令牌，通过 forwarder 鉴权）。滚动重启网关后 OAuth 刷新与模型请求均走带鉴权出口。

## Alternatives considered

1. **只依赖节点防火墙**：不采纳。防火墙不能提供请求级身份、令牌吊销和审计，且无法约束同节点其他租户。
2. **只在 engine 增加认证**：不采纳。独立 forwarder 是公网入口，必须在转发前阻断匿名请求，不能把信任交给后端回环豁免。
3. **复用集群 HMAC 票证作为客户端凭据**：不采纳。集群票证是节点间协议，暴露给业务 Pod 会扩大横向移动权限。
4. **默认关闭非回环监听**：保留作为运维默认，但不满足已有分布式 pproxy 的独立远端接入需求；显式开启远端监听时必须伴随客户端令牌。
5. **代理 URL 不带凭据、由节点 ACL 兜底**：不采纳。pod 侧 reqwest 无自定义代理头注入通道，ACL 无法区分同集群内多租户；URL userinfo → Basic 是 reqwest 原生通道，与 forwarder 的 Basic 校验天然对齐。

## Consequences

- **Positive**：
  - 腾讯节点 8899 对外形成强制鉴权边界：无凭据/错凭据一律 407（实测 `empty→407`、`wrong-token→407`、`correct-token→200`、`basic-token→200`）；
  - 红队 BLOCK 的三项解除条件（客户端鉴权覆盖 CONNECT/HTTP、节点出口可达、ponyllm Pod 鉴权 egress 实测通过）已满足：pod 内经 ClusterIP 带 token 建立 CONNECT 成功、无 token 407；OAuth 端点经代理返回真实 Google 401；`gemini-3.8-flash-high` 连续请求全部 HTTP 200；
  - 腾讯节点 pproxy 由 systemd 托管，重启自愈，不再依赖手动孤儿进程。
- **Negative / 待办**：
  - URL 内嵌 Basic 凭据可能进入错误日志，ponyllm 侧需确保脱敏（当前 reqwest 不打印 URL 明文，靠 review 持续盯）；
  - 节点防火墙/安全组 ACL 尚未按 `pproxy-service.md` 验收第 3 条补拒绝测试（同集群非授权租户访问必须被拒），标记为待补项；
  - token 目前硬编码于 systemd unit 与 k3s secret，后续应改为密钥管理轮转机制。
