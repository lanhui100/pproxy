# ADR: 移动端与桌面端 Clash 扫码全网漫游与直接注入方案优化

Status: implemented

## Problem

用户在移动端扫描桌面客户端设置中的 Clash 二维码时遇到 `...EOF` 报错。
根因分析：
1. **网络隔离与硬编码局域网**：桌面端生成的二维码内容为 `http://192.168.x.x:8899/clash.yaml`。手机在公网（蜂窝移动网络）或非同一局域网下根本无法访问该内网地址，HTTP 握手直接断开或被超时丢弃，导致客户端报 `unexpected EOF`；
2. **节点出口硬编码**：即使在局域网内拉取到配置，节点 `server` 依然是 `192.168.x.x:8899`，手机一旦离开办公室/家里 Wi-Fi，代理立刻瘫痪；
3. **扫码体验断层**：标准二维码若直接存放全量 YAML 配置（~3KB），会生成极其密集的二维码（Version 25+），手机相机极难扫码识别；同时 Clash Meta / Mihomo 客户端扫码时若不是标准 URL，无法通过常规订阅通道直接拉取。

## Decision

**采取“路径 B+ 深度优化架构”（公网订阅端点 + 标准 DeepLink Scheme + 隧道直连）：**

1. **服务端（Gate-Server）提供公网订阅端点**：
   - 在 `crates/gate-server/src/lib.rs` 中开放 `GET /clash` 与 `GET /api/clash`；
   - 支持通过 Query 参数 `?token=<token>` 获取针对该用户的完整 Clash YAML 配置；
   - 导出的配置中，代理节点自动对齐公网 Gate 隧道（`type: ws`, `server: <gate_host>`, `port: 443`, `Authorization: Bearer <token>`），不再回指局域网内网 IP。
2. **桌面端配置生成与端点自对齐**：
   - 桌面端检测当前是否配置了公网端点（默认 `wss://rn.ponygo.fun/ws`，或用户自定义的 gate_url）；
   - 若存在公网端点且存在有效 token，二维码优先生成公网订阅地址：`https://<gate_host>/api/clash?token=<token>`；
   - 同时支持生成标准 DeepLink URL Scheme（如 `clash://install-config?url=...`），手机扫码或浏览器一键打开直接唤起 Clash 客户端自动配置。
3. **退避兼容**：
   - 当无公网隧道时，自动回落至局域网 HTTP 模式并给出明确的同一 Wi-Fi 连接指引。

## Alternatives considered

- **纯二维码打包全量 YAML**：二维码承载上限一般为 2KB-3KB，过于密集导致扫码极度困难甚至扫码器报错解析失败；而且大多数手机 Clash 扫码只接受 URL。因此采用公网可达订阅 URL + DeepLink 是工业界最成熟可靠的方案。
- **让用户在手机上每次手动下载配置文件再导入**：用户体验差，无法享受动态规则与额度漫游。

## Consequences

- 手机扫码无论在局域网还是 4G/5G 蜂窝公网，均能直接 200 拉取订阅，彻底消除 `...EOF` 报错；
- 代理节点全部直连公网 Gate 隧道（WSS），手机端流量实时通过用户的专属份额与配额结算；
- 桌面端 UI 提示更清晰，提供订阅 URL、DeepLink 唤起以及二维码多重保障。
