# Pony Proxy — PRD

> 版本: v0.1 | 日期: 2026-08-21 | 状态: 已确认

## 1. 定位

自托管的**个人智能代理网关**：让国内设备（PC/手机）无障碍访问海外 API 服务（LLM 优先），用户友好、零信用卡成本、按需代理（不影响国内流量）。

## 2. 用户故事

- 作为用户，我打开 Pony Proxy 面板，一眼看到所有服务路由的健康状态和用量。
- 作为用户，我一键添加新服务（如 Gemini），系统自动选择上游并生效。
- 作为用户，我为每台设备生成独立 token；设备丢失时一键撤销。
- 作为用户，我一键复制某服务的接入配置（base_url + token），粘贴即用。
- 作为用户，任一上游用量超 80% 时收到告警。
- 作为用户，我在外网/手机上也能使用和管理（经公网入口）。

## 3. 功能需求

### P0 — MVP
| 模块 | 需求 |
|------|------|
| 数据面 | token 鉴权（路径 token `/{token}/{route}/...` 为主，`X-Pony-Token` header 为辅）；路由分发（现有 7 服务）；CONNECT 直连兜底 |
| 管理面 | REST API：token CRUD、路由 CRUD、用量查询、健康检查、告警查询 |
| 路由白名单 | 内置常用服务模板（OpenAI/Anthropic/Gemini/OpenRouter/Groq/Mistral/xAI/GitHub/HF/zen）；**自动上游选择**（CF 敏感服务 → Vercel，其余 → Worker）+ 手动覆盖；可增删 |
| CLI | `pony status / start / stop / restart / route / token / usage / doctor / config export` |
| Windows GUI | Tauri 2 瘦客户端：Dashboard、Routes、Tokens、Usage 图表、Settings |
| 监控 | per-route/per-token 请求与字节计数 → SQLite（保留 30 天）；每小时轮询 CF/Vercel 官方限额 API；80% 阈值告警（GUI 通知 + webhook 预留） |

### P1
- CF Tunnel 公网入口集成（手机/外网接入，域名 `access.ponyjob.top`）
- 服务模板一键导入库扩充
- 告警渠道扩展（webhook / 邮件）

### P2
- Mobile（Tauri 2 iOS/Android）
- 自定义上游（用户自有 VPS / HTTP 代理）
- 多用户支持

## 4. 非目标（明确不做）

- 系统级全局代理 / TUN 设备 / MITM 解密（与上游 `?url=` 转发模式矛盾）
- 透明接管国内流量（按需代理，零影响国内访问）
- 多租户/商业化

## 5. 关键体验指标

- 新服务从添加到可用：< 1 分钟
- 新设备接入：复制一行 base_url 即用
- Dashboard 数据延迟：< 1 小时（限额）/ 实时（请求计数）
