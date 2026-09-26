# Agent Note: 修复桌面端多租户出口测速401与PAC直连误伤GitHub

Status: implemented

## Problem

用户反馈桌面端出现问题：出口 C 和出口 V 显示失败（报 401 Unauthorized），且 GitHub 站点测速失败（报 timeout 10s）。

根因排查：
1. **出口 C 和出口 V 失败 (401 Unauthorized)**：
   - 桌面端配置多租户自包含令牌（`usr_live_...`）后，`resolve_gate_url_for_iface` 在解析 `cf` 和 `vercel` 端点时，仍返回了仅认单口令哈希（`TUNNEL_TOKEN_HASH`）的 Node/CF 网关（`gate.ponygo.fun` 和 `vgate.ponygo.fun`）。
   - 当前集群中只有 Rust 原生网关（`rn.ponygo.fun`）配置了 `USER_VERIFYING_KEY` 支持多租户 Ed25519 签名验证。因此向 CF/Vercel 发起 WebSocket 鉴权握手时被对方直接拒绝（HTTP 401）。
2. **GitHub 站点测速失败 (timeout 10s)**：
   - 在 `desktop/src-tauri/src/proxy/pac.rs` 中，`collect_bypass_hosts` 将 `tauri.conf.json` 中配置的自更新端点加入到了全局绕过列表（bypass hosts）。
   - 自更新端点中包含了 `https://github.com/lanhui100/pproxy/releases/latest/download/latest.json`，导致 `github.com` 被误提取并加入了 PAC 直连名单。
   - 本地引擎分流函数 `decide()` 在做分流判定时，命中 bypass 名单的域名会被强制走 `Route::Direct` 直连出网。在国内网络直连 `github.com:443` 极易受阻甚至握手超时（10秒拨测耗尽），而未走隧道代理加速。

## Decision

1. **出口解析感知多租户令牌并收敛至有效端点**：
   - 在 `desktop/src-tauri/src/lib.rs` 的 `resolve_gate_url_for_iface` 中，增加令牌类型检测（`t.starts_with("usr_live_")`）。
   - 当使用多租户令牌时，所有物理出口接口（出口 R、出口 C、出口 V）测速均统一定向至已启用 Ed25519 非对称验签的 Rust 原生出口（`rn.ponygo.fun`），避免向单口令网关发起无效握手导致 401 失败红字。
2. **清理 PAC bypass 名单中的 github.com 污染**：
   - 修改 `desktop/src-tauri/src/proxy/pac.rs` 中的 `collect_bypass_hosts` 静态列表，移除 `github.com` 更新端点，仅保留私有分发域名（`dl.ponygo.fun`、`access.ponygo.fun`、`access.example.com`）。
   - 确保 `github.com` 正常命中默认种子白名单并通过 WS 隧道加速出网，解除 10s 超时困境。
3. **版本迭代与发布**：
   - 桌面端版本升级为 `0.3.60`。

## Alternatives considered

- **在 CF Worker 和 Vercel Gate-Worker 增加多租户 Ed25519 签名验证**：需要向所有无服务器环境分发密钥对，工程链路长；当前全部出海隧道流量已收敛由高带宽原生 VPS（rn）统一承载，复用可用端点测速成本最低且最可靠。
- **让 GitHub 站点测速绕过本地引擎直接测速**：违背 P2-5 原则（「链接状态 = 实际可用性」），无法反映真实代理出网状态；修复 PAC bypass 污染是根本解法。

## Consequences

- 出口 R、出口 C、出口 V 在多租户与单口令模式下均能正常获取延迟绿条，消除了 401 报错。
- `github.com` 经由智能分流正常走 WS 隧道加速，拨测延迟恢复在 1s 左右。
- 门禁全部通过（Rust 71 单测通过，前端 119 单测全部通过）。
