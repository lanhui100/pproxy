# ADR-003: 子域名避开 "proxy" 敏感词

- 状态: 已接受 | 日期: 2026-08-21

## 背景

`proxy.example.com` 在 CF 配置正确、海外 DNS（8.8.8.8/1.1.1.1/CF NS）解析正常的情况下，国内 DNS（阿里 223.5.5.5、腾讯 119.29.29.29、本地网关）持续返回 **NXDOMAIN**。刷新缓存无效，判定为国内 DNS 对敏感词子域名的过滤（"proxy" 命中）。

## 决策

所有对外子域名避开敏感词，采用中性命名：

| 子域名 | 用途 |
|--------|------|
| edge.example.com | CF Worker 入口 |
| vedge.example.com | Vercel 函数入口 |
| access.example.com | 未来 CF Tunnel 公网入口（规划） |

## 后果

- ✅ 国内 DNS 全部正常解析（edge/vedge 已验证）
- ❌ "proxy" 字样域名永久不可用
- 临时方案 /etc/hosts 硬编码已废弃（hosts 方案对 IP 变更脆弱）
