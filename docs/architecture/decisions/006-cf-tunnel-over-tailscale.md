# ADR-006: 公网入口采用 CF Tunnel（Tailscale 方案否决）

- 状态: 已接受 | 日期: 2026-08-22

## 背景

M4 需要把数据面暴露到公网（手机 4G 下 SDK 可用）。候选方案：Cloudflare Tunnel 与 Tailscale（后者已在 dev 服务器运行，tailnet 内可达）。

本项目最硬约束是**国内运营商网络可达性**——ADR-002/003 的全部选型都围绕它展开。

## 决策

数据面公网入口采用 **CF Tunnel**：`access.ponyjob.top`（ADR-003 预留的中性命名）→ cloudflared → `http://127.0.0.1:8899`，systemd 单元 `pony-tunnel.service` 托管，协议钉死 http2（规避大陆 UDP 劣化），ingress 仅此一条规则。

**否决 Tailscale 作为公网入口替代**，理由：

1. 大陆无官方 DERP 中继节点，直连质量取决于运营商 NAT 穿透运气，回落海外中继丢包/被扰常见——与否决它的同一教训适用于本场景
2. 手机端需安装客户端并登录 tailnet 才能路由，验收场景"任何 4G 手机直接访问 HTTPS URL"不成立
3. 隧道出站连接质量不可控时表现为反复重建，最难排查

Tailscale 保留现状：仅作个人运维通道（tailnet 内 SSH 触达管理面），不写入架构拓扑、不承担任何验收路径。

## 后果

- ✅ 复用已被生产验证的 CF 边缘国内可达路径（与 edge/vedge 同源）
- ✅ 服务器零入站端口；TLS 由 CF 边缘终结
- ✅ 零客户端安装，SDK 直接配 base_url
- ⚠️ **TLS 在 CF 边缘终结：LLM prompt/completion 明文对 Cloudflare 可见**——个人项目显式接受的取舍
- ⚠️ token 进入 URL path，新增泄露渠道（CF 边缘日志、手机端 SDK 日志/剪贴板）；缓解事实：token 128bit 熵不可枚举 + 401 同体防枚举
- ⚠️ per-token 限速维持 P1，裸奔窗口期风险显式接受；滥用升级路径 = CF 免费层 rate limiting rule
- ⚠️ CF 免费层平台限制对 Tunnel 同样生效：边缘等待 ~100s → 524、请求体 ~100MB 上限；长任务必须走 streaming
- ⚠️ 运维红线：本机 WARP 客户端若 connect 会把 cloudflared 出站连接卷入 WARP 隧道，两者互斥
