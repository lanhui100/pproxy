# ADR-002: 双上游架构（CF Worker + Vercel Function）

- 状态: 已接受 | 日期: 2026-08-21

## 背景

需要免费、无需国外信用卡、国内可达的海外出口。实测结论：

| 出口 | 国内可达 | 问题 |
|------|---------|------|
| CF Worker (`*.workers.dev`) | ❌ workers.dev 被墙 | 需绑自定义域名（已解） |
| CF Worker 出口 IP | — | OpenAI 按 AS13335 整段拉黑；CF→CF 内部流量传播入口国家（zen 返回 RegionError） |
| Vercel Function 出口 | vercel.app 被墙 | 绑自定义域名（已解）；出口 AWS 真实 IP，OpenAI/zen 均放行 |
| HF Space | — | 免费层已不含 Docker（见 ADR-004） |

另发现：CF Worker 转发时若保留 `x-forwarded-for` 等头会泄露客户端真实 IP（中国）导致地区拦截，已在 Worker/函数中剥离。

## 决策

双上游按服务分流：

- **CF Worker（默认）**：anthropic / google / github / x / facebook——不查数据中心 IP
- **Vercel（敏感服务）**：openai / opencode——需要"干净"出口

## 后果

- ✅ 全部目标服务可达，零成本
- ✅ 单上游故障可按路由切换
- ⚠️ Vercel Hobby 函数上限 300s（Fluid compute）
- ⚠️ 新增服务需判断上游敏感度（route test 实测）
- ⚠️ 依赖两家免费额度（CF 10 万 req/天、Vercel 100GB/月，个人用量余量极大）
